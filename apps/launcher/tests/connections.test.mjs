import assert from "node:assert/strict";
import test from "node:test";
import { connections } from "../src/connections.ts";

const app = { id: "firefox", name: "Firefox", description: "", source: "desktop", icon_png: "app-icon" };
const window = (id, extra = {}) => ({ id, name: "Firefox", title: "搜索页", window_id: 100, controlled: false, minimized: false, ...extra });
const session = (session_id, app_id, extra = {}) => ({ session_id, app_id, backend: "native", display: 0, controlled: false, ...extra });

test("a connected desktop window and its resumable session share one row", () => {
  const result = connections([app], [window("window-1", { session_id: "s1", icon_png: "window-icon" })], [session("s1", "window-1")]);
  assert.equal(result.length, 1);
  assert.equal(result[0].sessionId, "s1");
  assert.equal(result[0].title, "搜索页");
  assert.equal(result[0].icon_png, "window-icon");
  assert.equal(result[0].kind, "desktop");
  assert.equal(result[0].controlled, false);
});

test("independent remote instances are preserved alongside an untouched desktop window", () => {
  const result = connections([app], [window("desktop-window")], [session("s1", "firefox", { display: 200 }), session("s2", "firefox", { display: 201 })]);
  assert.equal(result.length, 3);
  const remote = result.filter(r => r.kind === "remote");
  assert.equal(remote.length, 2);
  assert.deepEqual(new Set(remote.map(r => r.sessionId)), new Set(["s1", "s2"]));
  assert.ok(remote.every(r => r.name === "Firefox" && r.icon_png === "app-icon"));
  assert.equal(result.find(r => r.appId === "desktop-window").sessionId, null);
});

test("partially refreshed metadata still resumes known sessions and respects control ownership", () => {
  const fallback = connections([], [window("window-1")], [session("s1", "window-1", { controlled: true })]);
  assert.equal(fallback.length, 1);
  assert.equal(fallback[0].sessionId, "s1");
  assert.equal(fallback[0].controlled, true);
  const windowOnly = connections([], [window("window-1", { session_id: "s1" })], []);
  assert.equal(windowOnly[0].sessionId, "s1");
  const multiple = connections([], [window("window-1", { session_id: "s1" }), window("window-2", { session_id: "s1", controlled: true })], [session("s1", "custom-existing")]);
  assert.equal(multiple.length, 1);
  assert.equal(multiple[0].controlled, true);
});
