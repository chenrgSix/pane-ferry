import assert from "node:assert/strict";
import test from "node:test";
import { CatalogCache, readCatalog } from "../src/catalog-cache.ts";
import { endpointKey } from "../src/launcher-types.ts";

const app = (id, name = id) => ({ id, name, description: "应用", source: "desktop", icon_png: "iVBORw0KGgo=" });
function memory() {
  const data = new Map();
  return { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
}

test("names and icons survive restart; cached reads do not call Ubuntu until explicit refresh", async () => {
  const storage = memory();
  let calls = 0;
  const fetch = async () => ({ apps: [app("firefox", "火狐")], discovery_error: null, calls: ++calls });
  const key = endpointKey({ peerId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" });
  const first = await readCatalog(new CatalogCache(storage), key, fetch);
  assert.equal(first.fromCache, false);
  assert.equal(first.persisted, true);
  const cache = new CatalogCache(storage);
  const second = await readCatalog(cache, key, fetch);
  assert.equal(second.fromCache, true);
  assert.deepEqual(second.catalog.apps, first.catalog.apps);
  assert.equal(second.savedAt, first.savedAt);
  assert.equal(calls, 1);
  const refreshed = await readCatalog(cache, key, async () => ({ apps: [app("settings")], discovery_error: null }), true);
  assert.deepEqual(refreshed.catalog.apps.map(a => a.id), ["settings"]);
  assert.deepEqual(new CatalogCache(storage).get(key).apps, refreshed.catalog.apps);
});

test("paired devices have isolated catalogs", async () => {
  const cache = new CatalogCache(memory());
  const keys = [
    { peerId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" },
    { peerId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" },
    { peerId: "cccccccc-cccc-cccc-cccc-cccccccccccc" },
  ].map(endpointKey);
  for (const [i, key] of keys.entries()) {
    const result = await readCatalog(cache, key, async () => ({ apps: [app(`app-${i}`)], discovery_error: null }));
    assert.equal(result.fromCache, false);
  }
  for (const [i, key] of keys.entries()) assert.equal(cache.get(key).apps[0].id, `app-${i}`);
});

test("failed or partial discovery preserves the last successful catalog", async () => {
  const storage = memory();
  const cache = new CatalogCache(storage);
  cache.put("host", [app("firefox")]);
  await assert.rejects(readCatalog(cache, "host", async () => { throw new Error("offline"); }, true), /offline/);
  const result = await readCatalog(cache, "host", async () => ({ apps: [], discovery_error: "unavailable" }), true);
  assert.equal(result.catalog.discovery_error, "unavailable");
  assert.equal(result.catalog.apps[0].id, "firefox");
  assert.equal(new CatalogCache(storage).get("host").apps[0].id, "firefox");
  const empty = await readCatalog(cache, "other", async () => ({ apps: [], discovery_error: "unavailable" }));
  assert.equal(empty.savedAt, null);
  assert.equal(cache.get("other"), undefined);
});

test("invalid or unavailable storage falls back to a usable in-memory cache", async () => {
  for (const storage of [
    { getItem: () => "{broken", setItem() {} },
    { getItem: () => JSON.stringify([["host", { savedAt: 1, apps: [{}] }]]), setItem() {} },
    { getItem() { throw new Error("denied"); }, setItem() { throw new Error("quota"); } },
  ]) {
    const cache = new CatalogCache(storage);
    assert.equal(cache.get("host"), undefined);
    await readCatalog(cache, "host", async () => ({ apps: [app("firefox")], discovery_error: null }));
    const next = await readCatalog(cache, "host", async () => { throw new Error("must not read remote"); });
    assert.equal(next.catalog.apps[0].id, "firefox");
  }
});
