import type { AppCatalog, AppInfo } from "./launcher-types";

type StorageLike = {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
};
type Snapshot = { apps: AppInfo[]; savedAt: number };
export type CatalogResult = {
  catalog: AppCatalog;
  savedAt: number | null;
  fromCache: boolean;
  persisted: boolean;
};
const storageKey = "lan-remoteapp.app-catalogs.v1";
// Leave room for connection preferences and pending launch IDs in WebView storage.
const maxCharacters = 2 * 1024 * 1024;
const maxHosts = 8;
function validApp(value: unknown): value is AppInfo {
  if (!value || typeof value !== "object") return false;
  const app = value as AppInfo;
  return typeof app.id === "string" && /^[A-Za-z0-9_-]{1,100}$/.test(app.id)
    && typeof app.name === "string" && app.name.length <= 1024
    && typeof app.description === "string" && app.description.length <= 4096
    && (app.source === "desktop" || app.source === "custom")
    && (app.unavailable_reason == null || (typeof app.unavailable_reason === "string" && app.unavailable_reason.length <= 4096))
    && (app.icon_png == null || (typeof app.icon_png === "string" && app.icon_png.length <= 24 * 1024 && /^[A-Za-z0-9+/]*={0,2}$/.test(app.icon_png)));
}

export class CatalogCache {
  private readonly entries = new Map<string, Snapshot>();
  private readonly persisted = new Set<string>();
  private readonly storage?: StorageLike;

  constructor(storage?: StorageLike) {
    this.storage = storage;
    try {
      const raw = storage?.getItem(storageKey);
      if (!raw || raw.length > maxCharacters) return;
      const values: unknown = JSON.parse(raw);
      if (!Array.isArray(values)) return;
      for (const entry of values.slice(-maxHosts)) {
        if (!Array.isArray(entry) || entry.length !== 2) continue;
        const [key, value] = entry;
        if (typeof key !== "string" || key.length > 2048 || !value
          || !Number.isSafeInteger(value.savedAt) || value.savedAt <= 0
          || !Array.isArray(value.apps) || value.apps.length > 1024
          || !value.apps.every(validApp)) continue;
        this.entries.set(key, value);
        this.persisted.add(key);
      }
    } catch {
      // Storage can be disabled or left over from an interrupted write.
    }
  }

  get(key: string): Snapshot | undefined {
    return this.entries.get(key);
  }

  isPersisted(key: string): boolean {
    return this.persisted.has(key);
  }

  put(key: string, apps: AppInfo[]): Snapshot {
    const snapshot = { apps, savedAt: Date.now() };
    this.entries.delete(key);
    this.entries.set(key, snapshot);
    while (this.entries.size > maxHosts) this.entries.delete(this.entries.keys().next().value!);
    const values = [...this.entries];
    let raw = JSON.stringify(values);
    while (raw.length > maxCharacters && values.length > 1) {
      const removed = values.shift()!;
      this.entries.delete(removed[0]);
      raw = JSON.stringify(values);
    }
    this.persisted.clear();
    try {
      if (raw.length <= maxCharacters && this.storage) {
        this.storage.setItem(storageKey, raw);
        for (const [host] of values) this.persisted.add(host);
      }
    } catch {
      // Keep the current list usable even if persistence is unavailable.
    }
    return snapshot;
  }
}

export async function readCatalog(
  cache: CatalogCache,
  key: string,
  fetch: () => Promise<AppCatalog>,
  refresh = false,
): Promise<CatalogResult> {
  const cached = cache.get(key);
  if (cached && !refresh) return {
    catalog: { apps: cached.apps, discovery_error: null },
    savedAt: cached.savedAt, fromCache: true, persisted: cache.isPersisted(key),
  };
  const catalog = await fetch();
  // A discovery failure may contain only custom entries; never erase a good list.
  if (catalog.discovery_error) return {
    catalog: { ...catalog, apps: cached?.apps ?? catalog.apps },
    savedAt: cached?.savedAt ?? null, fromCache: Boolean(cached), persisted: cache.isPersisted(key),
  };
  const snapshot = cache.put(key, catalog.apps);
  return { catalog, savedAt: snapshot.savedAt, fromCache: false, persisted: cache.isPersisted(key) };
}
