import assert from "node:assert/strict";
import test from "node:test";
import { PendingLaunches } from "../src/session-store.ts";

function memory() {
  const data = new Map();
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, value),
  };
}

test("a lost reply keeps the same request ID across launcher restarts", () => {
  const storage = memory();
  const first = new PendingLaunches(storage).id("device-a:xterm:native");
  assert.equal(
    new PendingLaunches(storage).id("device-a:xterm:native"),
    first,
  );
  const retry = new PendingLaunches(storage);
  retry.resolve("device-a:xterm:native");
  assert.notEqual(
    new PendingLaunches(storage).id("device-a:xterm:native"),
    first,
  );
});

test("different devices and apps do not share a request", () => {
  const pending = new PendingLaunches(memory());
  const keys = ["a:x:native", "b:x:native", "a:y:native", "c:x:native"];
  assert.equal(new Set(keys.map((key) => pending.id(key))).size, keys.length);
});

test("damaged or unavailable storage does not break an in-memory retry", () => {
  for (const storage of [
    { getItem: () => "{broken", setItem() {} },
    {
      getItem() {
        throw new Error("blocked");
      },
      setItem() {
        throw new Error("blocked");
      },
    },
  ]) {
    const pending = new PendingLaunches(storage);
    assert.equal(pending.id("same"), pending.id("same"));
  }
});
