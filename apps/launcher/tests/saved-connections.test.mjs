import assert from "node:assert/strict";
import test from "node:test";
import { SavedConnections, connectionAddress } from "../src/saved-connections.ts";

const peerId = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const row = { peerId, name: "家里的 Windows", address: "192.168.8.20:42424" };
function storage() {
  const data = new Map();
  return { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
}
test("saved devices survive restart and edits replace the same device without storing invitations", () => {
  const disk = storage(), store = new SavedConnections(disk);
  store.save({ ...row, invitation: "never-save-this" });
  store.save({ peerId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", name: "Mac", address: "192.168.8.11:42424" });
  store.save({ ...row, name: "办公电脑", address: "10.0.0.2:42424" });
  const restarted = new SavedConnections(disk);
  assert.equal(restarted.list().length, 2);
  assert.deepEqual(restarted.list()[0], { ...row, name: "办公电脑", address: "10.0.0.2:42424" });
  restarted.remove(peerId);
  assert.equal(new SavedConnections(disk).list().length, 1);
});
test("failed writes preserve existing records and never report an in-memory-only save", () => {
  const disk = storage(), store = new SavedConnections(disk);
  store.save(row);
  disk.setItem = () => { throw new Error("disk full"); };
  assert.throws(() => store.save({ ...row, name: "changed" }), /保存失败/);
  assert.throws(() => store.remove(peerId), /保存失败/);
  assert.deepEqual(store.list(), [row]);
  assert.throws(() => new SavedConnections().save(row), /保存失败/);
});
test("invalid persisted records are ignored without losing valid devices", () => {
  const store = new SavedConnections({ getItem: () => JSON.stringify([null, {}, row, { ...row, peerId: "host" }, row]), setItem() {} });
  assert.deepEqual(store.list(), [row]);
  assert.deepEqual(new SavedConnections({ getItem: () => "{", setItem() {} }).list(), []);
});
test("only explicit IP socket addresses are accepted, including bracketed IPv6", () => {
  assert.equal(connectionAddress(" 192.168.8.20:42424 "), row.address);
  assert.equal(connectionAddress("[2001:0db8::1]:42424"), "[2001:db8::1]:42424");
  for (const address of ["localhost:42424", "127.1:42424", "256.0.0.1:42424", "1.2.3.4:0", "1.2.3.4:65536", "[abcd::zz]:42424", "1.2.3.4:42424/path", "1.2.3.4"]) {
    assert.throws(() => connectionAddress(address), undefined, address);
  }
});
