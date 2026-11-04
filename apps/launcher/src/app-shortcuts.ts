import { validPeerId } from "./launcher-types.ts";

export type AppShortcuts = { peerId: string; favorites: string[]; recent: string[] };
type StorageLike = Pick<Storage, "getItem" | "setItem">;
const key = "paneferry.app-shortcuts.v1";
const validApp = (id: unknown): id is string => typeof id === "string" && id.length > 0 && id.length <= 1024;
const ids = (value: unknown, limit: number): string[] => Array.isArray(value)
  ? [...new Set(value.filter(validApp))].slice(0, limit) : [];

// Only stable application IDs are kept locally; window titles and contents are
// not part of shortcuts. Device UUIDs keep equal app IDs on two peers separate.
export class AppShortcutStore {
  private entries: AppShortcuts[] = [];
  private storage: StorageLike | undefined;
  constructor(storage?: StorageLike) {
    this.storage = storage;
    try {
      const raw = storage?.getItem(key);
      if (!raw || raw.length > 512 * 1024) return;
      const parsed: unknown = JSON.parse(raw);
      if (!Array.isArray(parsed)) return;
      for (const row of parsed.slice(0, 100)) {
        if (!row || !validPeerId(row.peerId) || this.entries.some(item => item.peerId === row.peerId)) continue;
        this.entries.push({ peerId: row.peerId, favorites: ids(row.favorites, 100), recent: ids(row.recent, 8) });
      }
    } catch { /* Shortcuts must never prevent the application from opening. */ }
  }
  list(): AppShortcuts[] { return this.entries.map(row => ({ ...row, favorites: [...row.favorites], recent: [...row.recent] })); }
  toggleFavorite(peerId: string, appId: string): void {
    this.update(peerId, row => {
      if (row.favorites.includes(appId)) row.favorites = row.favorites.filter(id => id !== appId);
      else {
        if (row.favorites.length >= 100) throw new Error("每台设备最多收藏 100 个应用，请先取消一些收藏。");
        row.favorites.push(appId);
      }
    }, appId);
  }
  recordUse(peerId: string, appId: string): void {
    this.update(peerId, row => { row.recent = [appId, ...row.recent.filter(id => id !== appId)].slice(0, 8); }, appId);
  }
  private update(peerId: string, mutate: (row: AppShortcuts) => void, appId: string): void {
    if (!validPeerId(peerId) || !validApp(appId)) throw new Error("应用快捷入口无效，请刷新后重试。");
    const entries = this.list();
    let row = entries.find(item => item.peerId === peerId);
    if (!row) {
      if (entries.length >= 100) throw new Error("保存的快捷入口设备已达到上限。");
      row = { peerId, favorites: [], recent: [] }; entries.push(row);
    }
    mutate(row);
    try {
      if (!this.storage) throw new Error("storage unavailable");
      const serialized = JSON.stringify(entries);
      if (serialized.length > 512 * 1024) throw new Error("shortcuts too large");
      this.storage.setItem(key, serialized);
    } catch { throw new Error("应用快捷入口保存失败，原记录已保留。请检查本机存储后重试。"); }
    this.entries = entries;
  }
}
