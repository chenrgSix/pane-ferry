import { validPeerId } from "./launcher-types.ts";

type Preferences = Pick<Storage, "getItem" | "setItem" | "removeItem">;

// Retain paired-device caches and ambiguous launch IDs, never reinterpret a
// legacy host/account as a paired identity. This is safe to repeat on startup.
export function retireLegacyPreferences(storage?: Preferences) {
  if (!storage) return;
  try {
    storage.removeItem("lan-remoteapp.devices");
    const saved = JSON.parse(storage.getItem("lan-remoteapp.connection") || "null");
    if (saved) {
      const endpoint = validPeerId(saved.endpoint?.peerId) ? {peerId: saved.endpoint.peerId} : {peerId: ""};
      storage.setItem("lan-remoteapp.connection", JSON.stringify({endpoint, applicationScene: saved.applicationScene === true}));
    }
    for (const key of ["lan-remoteapp.app-catalogs.v1", "lan-remoteapp.pending-launches"]) {
      const raw = storage.getItem(key);
      if (!raw) continue;
      if (raw.length > 2 * 1024 * 1024) { storage.removeItem(key); continue; }
      const entries: unknown = JSON.parse(raw);
      if (!Array.isArray(entries)) { storage.removeItem(key); continue; }
      const kept = entries.filter(entry => {
        if (!Array.isArray(entry) || typeof entry[0] !== "string") return false;
        try {
          const identity = JSON.parse(entry[0].slice(0, entry[0].indexOf("}") + 1));
          return identity.transport === "quic" && validPeerId(identity.deviceId);
        } catch { return false; }
      });
      storage.setItem(key, JSON.stringify(kept));
    }
  } catch { /* Storage failure cannot prevent pairing or in-memory operation. */ }
}
