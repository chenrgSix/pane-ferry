type StorageLike = {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
};
const storageKey = "lan-remoteapp.pending-launches";

// Keep ambiguous launches stable even when the launcher closes before a reply.
export class PendingLaunches {
  private readonly storage?: StorageLike;
  private readonly pending = new Map<string, string>();

  constructor(storage?: StorageLike) {
    this.storage = storage;
    try {
      const raw = storage?.getItem(storageKey);
      if (!raw || raw.length > 512 * 1024) return;
      const entries: unknown = JSON.parse(raw);
      if (!Array.isArray(entries)) return;
      for (const entry of entries.slice(0, 256)) {
        if (
          Array.isArray(entry) &&
          entry.length === 2 &&
          typeof entry[0] === "string" &&
          entry[0].length <= 2048 &&
          typeof entry[1] === "string" &&
          /^[A-Za-z0-9_-]{1,100}$/.test(entry[1])
        ) {
          this.pending.set(entry[0], entry[1]);
        }
      }
    } catch {
      /* Storage may be unavailable; retain in-memory retries. */
    }
  }

  id(key: string): string {
    let id = this.pending.get(key);
    if (!id) {
      if (this.pending.size >= 256)
        throw new Error("未确认的启动过多，请先检查并恢复远端会话。");
      id = crypto.randomUUID();
      this.pending.set(key, id);
      this.save();
    }
    return id;
  }

  resolve(key: string): void {
    this.pending.delete(key);
    this.save();
  }

  private save(): void {
    try {
      this.storage?.setItem(storageKey, JSON.stringify([...this.pending]));
    } catch {}
  }
}
