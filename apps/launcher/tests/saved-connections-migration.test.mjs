import test from "node:test";
import assert from "node:assert/strict";
import { SavedConnections } from "../src/saved-connections.ts";

const peerId = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const row = { peerId, name: "办公电脑", address: "192.168.8.20:42424" };
const key = "paneferry.saved-connections.v1";
function disk() {
  const data = new Map();
  return { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
}
test("legacy bookmarks import once, omit credentials, and never resurrect a removed bookmark", () => {
  const storage = disk(), store = new SavedConnections(storage);
  assert.equal(store.importLegacy([null, {}, { ...row, invitation: "not-a-bookmark" }, row]), true);
  assert.deepEqual(store.list(), [row]);
  assert.deepEqual(JSON.parse(storage.getItem(key)), [row]);
  store.remove(peerId);
  const restarted = new SavedConnections(storage);
  assert.equal(restarted.importLegacy([row]), false);
  assert.deepEqual(restarted.list(), []);
});
test("shared bookmarks take precedence and a failed import commits no partial state", () => {
  const storage = disk(), store = new SavedConnections(storage);
  store.save(row);
  assert.equal(store.importLegacy([{ ...row, name: "obsolete", address: "10.0.0.20:42424" }]), false);
  assert.deepEqual(store.list(), [row]);
  const unavailable = { getItem: () => null, setItem: () => { throw new Error("quota"); } };
  const failed = new SavedConnections(unavailable);
  assert.throws(() => failed.importLegacy([row]), /保存失败/);
  assert.deepEqual(failed.list(), []);
});
