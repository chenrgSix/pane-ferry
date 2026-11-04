import test from "node:test";
import assert from "node:assert/strict";
import { AppShortcutStore } from "../src/app-shortcuts.ts";

const first = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const second = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
const key = "paneferry.app-shortcuts.v1";
function storage(initial) {
  const data = new Map(initial ? [[key, JSON.stringify(initial)]] : []);
  return { getItem: id => data.get(id) ?? null, setItem: (id, value) => data.set(id, value) };
}

test("favorites and recent IDs persist independently for each paired device", () => {
  const disk = storage(), store = new AppShortcutStore(disk);
  store.toggleFavorite(first, "browser");
  store.recordUse(first, "browser");
  store.toggleFavorite(second, "editor");
  store.recordUse(second, "browser");
  assert.deepEqual(new AppShortcutStore(disk).list(), [
    { peerId: first, favorites: ["browser"], recent: ["browser"] },
    { peerId: second, favorites: ["editor"], recent: ["browser"] },
  ]);
  store.toggleFavorite(first, "browser");
  assert.deepEqual(store.list()[0], { peerId: first, favorites: [], recent: ["browser"] });
  const copy = store.list(); copy[1].favorites.push("external mutation");
  assert.deepEqual(store.list()[1].favorites, ["editor"]);
});

test("recent applications deduplicate, move to the front and remain bounded", () => {
  const store = new AppShortcutStore(storage());
  for (let i = 0; i < 12; i++) store.recordUse(first, `app-${i}`);
  store.recordUse(first, "app-6");
  assert.deepEqual(store.list()[0].recent, ["app-6", "app-11", "app-10", "app-9", "app-8", "app-7", "app-5", "app-4"]);
});

test("corrupt or oversized storage cannot introduce invalid devices or duplicate shortcuts", () => {
  const store = new AppShortcutStore(storage([null, {}, { peerId: "not-paired", favorites: ["app"] },
    { peerId: first, favorites: ["app", null, "", "app", "x".repeat(1025)], recent: ["app", "app", 3] },
    { peerId: first, favorites: ["duplicate peer"] }]));
  assert.deepEqual(store.list(), [{ peerId: first, favorites: ["app"], recent: ["app"] }]);
  assert.deepEqual(new AppShortcutStore({ getItem: () => "invalid JSON" }).list(), []);
  assert.deepEqual(new AppShortcutStore({ getItem: () => " ".repeat(512 * 1024 + 1) }).list(), []);
});

test("failed writes preserve the previous favorite and recent lists", () => {
  let fail = false;
  const disk = storage();
  const store = new AppShortcutStore({ getItem: disk.getItem, setItem: (key, value) => {
    if (fail) throw new Error("quota exceeded"); disk.setItem(key, value);
  } });
  store.toggleFavorite(first, "browser"); store.recordUse(first, "browser"); fail = true;
  assert.throws(() => store.toggleFavorite(first, "editor"), /原记录已保留/);
  assert.throws(() => store.recordUse(first, "editor"), /原记录已保留/);
  assert.deepEqual(store.list(), new AppShortcutStore(disk).list());
  assert.throws(() => store.toggleFavorite("host-name", "browser"), /无效/);
});
