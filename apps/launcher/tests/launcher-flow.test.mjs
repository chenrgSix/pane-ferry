import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { parse, compileScript } from "@vue/compiler-sfc";
import ts from "typescript";
import { createRenderer, nextTick } from "vue";

// Run the actual App.vue setup and mount/unmount hooks with Vue's non-DOM
// renderer. Only Tauri IPC and browser storage/focus are replaced by fixtures.
const source = new URL("../src/App.vue", import.meta.url);
const { descriptor } = parse(await readFile(source, "utf8"));
const script = compileScript(descriptor, { id: "launcher-flow" }).content
  .replace(/import \{ invoke \} from "@tauri-apps\/api\/core";/, "const invoke = (...args) => globalThis.launcherTestInvoke(...args);")
  .replace(/import AppIcon from "\.\/AppIcon.vue";/, "const AppIcon = {};")
  .replace(/from ["']vue["']/g, `from ${JSON.stringify(import.meta.resolve("vue"))}`)
  .replace(/from "(\.\/[^".]+(?:-[^".]+)*)"/g, (_, relative) => `from ${JSON.stringify(new URL(`${relative}.ts`, source).href)}`);
const scratch = await mkdtemp(join(tmpdir(), "remoteapp-launcher-flow-"));
const modulePath = join(scratch, "App.mjs");
await writeFile(modulePath, ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText);
const Component = (await import(pathToFileURL(modulePath).href)).default;
after(() => rm(scratch, { recursive: true, force: true }));

const renderer = createRenderer({
  createComment: text => ({ text }),
  insert(node, parent) { parent.child = node; node.parent = parent; },
  remove(node) { delete node.parent.child; },
  parentNode: node => node.parent,
  nextSibling: () => null,
});
const endpoint = { host: "ubuntu", user: "alice", port: 22 };
const app = { id: "firefox", name: "Firefox", source: "desktop", description: "浏览器" };
const session = (id, appId, display = 0, controlled = false) => ({ session_id: id, app_id: appId, display, controlled, backend: "native" });
const window = (id, sessionId = null, controlled = false) => ({ id, session_id: sessionId, controlled, name: "Firefox", title: "测试页面", minimized: false, window_id: 100 });
function storage() {
  const data = new Map([["lan-remoteapp.connection", JSON.stringify({ endpoint, renderScale: "auto" })]]);
  return { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
}
function mount(saved, fixtures = {}) {
  const calls = [];
  const listeners = new Map();
  globalThis.localStorage = saved;
  globalThis.window = { addEventListener: (type, callback) => listeners.set(type, callback), removeEventListener: type => listeners.delete(type) };
  globalThis.launcherTestInvoke = async (command, args) => {
    calls.push({ command, args });
    if (fixtures[command]) return fixtures[command](args);
    if (command === "list_apps") return { apps: [app], discovery_error: null };
    if (command === "list_sessions") return [session("remote", "firefox", 228)];
    if (command === "list_desktop_windows") return { windows: [], unavailable_reason: null };
    if (command === "launch_app" || command === "resume_session") return { session_id: args.sessionId || "new", display: "228", pid: 123, message: "客户端已启动" };
    throw new Error(`unexpected command ${command}`);
  };
  const instance = renderer.createApp({ ...Component, render: () => null });
  const vm = instance.mount({});
  return { state: vm.$.setupState, calls, listeners, unmount: () => instance.unmount() };
}
async function settled(state) {
  for (let i = 0; i < 50; i++) {
    await new Promise(resolve => setImmediate(resolve));
    if (!state.busy) return;
  }
  assert.fail("launcher did not settle");
}
const actions = mounted => mounted.calls.filter(c => c.command === "launch_app" || c.command === "resume_session");

test("actual launcher restores cached apps at mount and only refreshes live metadata on focus", async () => {
  const saved = storage();
  let mounted = mount(saved);
  try {
    await settled(mounted.state);
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 1);
    await mounted.state.load();
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 1);
  } finally { mounted.unmount(); }
  mounted = mount(saved);
  try {
    assert.equal(mounted.state.apps[0].name, "Firefox", "cached list is visible before IPC completes");
    await settled(mounted.state);
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 0);
    mounted.listeners.get("focus")();
    await settled(mounted.state);
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 0);
    assert.equal(mounted.calls.filter(c => c.command === "list_sessions").length, 2);
    await mounted.state.load(true);
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 1);
  } finally { mounted.unmount(); }
  assert.equal(mounted.listeners.size, 0);
});

test("one connection action resumes sessions, takes over untouched windows, and never relaunches on resume failure", async () => {
  let failResume = false;
  const mounted = mount(storage(), {
    list_sessions: () => [session("owned", "window-owned"), session("remote", "firefox", 228), session("busy", "window-busy", 0, true)],
    list_desktop_windows: () => ({ windows: [window("window-owned", "owned"), window("window-new"), window("window-busy", "busy", true)], unavailable_reason: null }),
    resume_session: ({ sessionId }) => {
      if (failResume) throw new Error("会话已退出");
      return { session_id: sessionId, display: "0", pid: 123, message: "已启动宿主" };
    },
  });
  try {
    const { state } = mounted;
    await settled(state);
    assert.equal(state.runningConnections.length, 4);
    await state.connect(state.runningConnections.find(r => r.sessionId === "owned"));
    assert.equal(actions(mounted).at(-1).command, "resume_session");
    assert.equal(actions(mounted).at(-1).args.sessionId, "owned");
    await state.connect(state.runningConnections.find(r => r.sessionId === "remote"));
    assert.equal(actions(mounted).at(-1).args.sessionId, "remote");
    await state.connect(state.runningConnections.find(r => r.appId === "window-new"));
    assert.equal(actions(mounted).at(-1).command, "launch_app");
    assert.equal(actions(mounted).at(-1).args.appId, "window-new");
    const before = actions(mounted).length;
    await state.connect(state.runningConnections.find(r => r.sessionId === "busy"));
    assert.equal(actions(mounted).length, before);
    failResume = true;
    await state.connect(state.runningConnections.find(r => r.sessionId === "owned"));
    assert.equal(actions(mounted).length, before + 1);
    assert.equal(actions(mounted).at(-1).command, "resume_session");
    assert.match(state.error, /会话已退出/);
  } finally { mounted.unmount(); }
});

test("catalog failures do not block live sessions; late metadata from a previous host is ignored", async () => {
  let rejectWindows;
  let deferWindows = false;
  const mounted = mount(storage(), {
    list_apps: () => { throw new Error("应用发现失败"); },
    list_desktop_windows: () => deferWindows ? new Promise((_, reject) => { rejectWindows = reject; }) : { windows: [], unavailable_reason: null },
  });
  try {
    const { state } = mounted;
    await settled(state);
    assert.equal(state.catalogCurrent, false);
    assert.equal(state.runtimeCurrent, true);
    await state.connect(state.runningConnections[0]);
    assert.equal(actions(mounted).at(-1).command, "resume_session");
    deferWindows = true;
    const pending = state.refresh();
    state.endpoint = { ...endpoint, host: "other-ubuntu" };
    await nextTick();
    rejectWindows(new Error("old host offline"));
    await pending;
    assert.equal(state.windowWarning, "");
    assert.equal(state.sessionWarning, "");
    assert.equal(state.runningConnections.length, 0);
  } finally { mounted.unmount(); }
});

test("focus refresh never swallows the first connection click or overwrites newer connection state", async () => {
  const now = Date.now;
  let defer = false;
  let controlled = false;
  const replies = [];
  const mounted = mount(storage(), {
    list_sessions: () => defer ? new Promise(resolve => replies.push(() => resolve([session("remote", "firefox", 228)]))) : [session("remote", "firefox", 228, controlled)],
    list_desktop_windows: () => defer ? new Promise(resolve => replies.push(() => resolve({ windows: [], unavailable_reason: null }))) : { windows: [], unavailable_reason: null },
  });
  try {
    const { state } = mounted;
    await settled(state);
    const later = now() + 6000;
    Date.now = () => later;
    defer = true;
    state.onFocus();
    state.onFocus();
    assert.equal(state.busy, false, "background metadata must leave the Connect action available");
    assert.equal(replies.length, 2, "duplicate focus events share one pending metadata read");
    defer = false;
    controlled = true;
    await state.connect(state.runningConnections[0]);
    assert.equal(actions(mounted).length, 1);
    assert.equal(actions(mounted)[0].command, "resume_session");
    assert.equal(state.runningConnections[0].controlled, true);
    for (const reply of replies) reply();
    await settled(state);
    assert.equal(state.runningConnections[0].controlled, true, "late pre-connection metadata must not replace the new status");
  } finally {
    Date.now = now;
    for (const reply of replies) reply();
    mounted.unmount();
  }
});
