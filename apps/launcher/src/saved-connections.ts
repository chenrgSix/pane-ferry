import { validPeerId } from "./launcher-types.ts";

export type SavedConnection = { peerId: string; name: string; address: string };
type StorageLike = Pick<Storage, "getItem" | "setItem">;
const key = "paneferry.saved-connections.v1";

export function connectionAddress(value: string): string {
  const address = value.trim();
  const match = /^(\[[0-9a-fA-F:.]+\]|[0-9.]+):([0-9]{1,5})$/.exec(address);
  if (!match || Number(match[2]) < 1 || Number(match[2]) > 65535)
    throw new Error("请填写有效的 IP:端口，例如 192.168.8.20:42424；IPv6 使用 [地址]:端口。");
  const host = match[1];
  if (!host.startsWith("[") && (host.split(".").length !== 4 || host.split(".").some(
    part => !/^(0|[1-9][0-9]{0,2})$/.test(part) || Number(part) > 255)))
    throw new Error("IP 地址无效，请检查后重试。");
  try { return `${new URL(`http://${host}/`).hostname}:${Number(match[2])}`; }
  catch { throw new Error("IP 地址无效，请检查后重试。"); }
}

function normalize(value: SavedConnection): SavedConnection {
  if (!validPeerId(value.peerId)) throw new Error("请先配对设备，再保存连接。");
  if (typeof value.name !== "string" || !value.name.trim() || value.name.trim().length > 80)
    throw new Error("请填写 1 至 80 个字符的连接名称。");
  return { peerId: value.peerId, name: value.name.trim(), address: connectionAddress(value.address) };
}

// Bookmarks contain no invitation or certificate and cannot create pairing trust.
export class SavedConnections {
  private entries: SavedConnection[] = [];
  private storage: StorageLike | undefined;
  constructor(storage?: StorageLike) {
    this.storage = storage;
    try {
      const raw = storage?.getItem(key);
      if (!raw || raw.length > 128 * 1024) return;
      const parsed: unknown = JSON.parse(raw);
      if (!Array.isArray(parsed)) return;
      for (const entry of parsed.slice(0, 100)) {
        try {
          const item = normalize(entry);
          if (!this.entries.some(row => row.peerId === item.peerId)) this.entries.push(item);
        } catch { /* Ignore corrupt entries individually. */ }
      }
    } catch { /* A corrupt bookmark file must not prevent normal pairing. */ }
  }
  list(): SavedConnection[] { return this.entries.map(row => ({ ...row })); }
  save(value: SavedConnection): void {
    const item = normalize(value);
    const exists = this.entries.some(row => row.peerId === item.peerId);
    if (!exists && this.entries.length >= 100) throw new Error("最多保存 100 台设备，请先移除不再使用的连接。");
    this.commit(exists ? this.entries.map(row => row.peerId === item.peerId ? item : row) : [...this.entries, item]);
  }
  remove(peerId: string): void { this.commit(this.entries.filter(row => row.peerId !== peerId)); }
  private commit(entries: SavedConnection[]): void {
    try {
      if (!this.storage) throw new Error("storage unavailable");
      this.storage.setItem(key, JSON.stringify(entries));
    } catch { throw new Error("连接保存失败，原记录已保留。请检查本机存储后重试。"); }
    this.entries = entries;
  }
}
