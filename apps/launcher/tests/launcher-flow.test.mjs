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
const scratch = await mkdtemp(join(tmpdir(), "paneferry-launcher-flow-"));
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
const endpoint = { peerId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" };
const app = { id: "firefox", name: "Firefox", source: "desktop", description: "浏览器", icon_png: "Y2FjaGVkLWFwcC1pY29u" };
const session = (id, appId, display = 0, controlled = false) => ({ session_id: id, app_id: appId, display, controlled, backend: "native" });
const window = (id, sessionId = null, controlled = false) => ({ id, session_id: sessionId, controlled, name: "Firefox", title: "测试页面", minimized: false, window_id: 100, icon_png: "bGl2ZS13aW5kb3ctaWNvbg==" });
function storage() {
  const data = new Map([["lan-remoteapp.connection", JSON.stringify({ endpoint, renderScale: "auto" })]]);
  return { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value), removeItem: key => data.delete(key) };
}
function mount(saved, fixtures = {}) {
  const calls = [];
  const listeners = new Map();
  globalThis.localStorage = saved;
  globalThis.window = { screen: {availWidth: 1470, availHeight: 837}, devicePixelRatio: fixtures.devicePixelRatio ?? 1,
    addEventListener: (type, callback) => listeners.set(type, callback), removeEventListener: type => listeners.delete(type) };
  globalThis.launcherTestInvoke = async (command, args) => {
    calls.push({ command, args });
    if (fixtures[command]) return fixtures[command](args);
    if (command === "list_apps") return { apps: [app], discovery_error: null };
    if (command === "list_sessions") return [session("remote", "firefox", 228)];
    if (command === "list_desktop_windows") return { windows: [], unavailable_reason: null };
    if (command === "list_desktop_monitors") return { monitors: [], unavailable_reason: null };
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
async function waitUntil(predicate, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate() && Date.now() < deadline)
    await new Promise(resolve => setTimeout(resolve, 25));
  assert.ok(predicate(), "launcher did not update after automatic startup polling");
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
    const runtimeReads = mounted.calls.filter(c => ["list_sessions", "list_desktop_windows", "list_desktop_monitors"].includes(c.command)).length;
    await mounted.state.load(true);
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 1);
    assert.equal(mounted.calls.filter(c => ["list_sessions", "list_desktop_windows", "list_desktop_monitors"].includes(c.command)).length, runtimeReads,
      "refreshing installed apps must not refresh sessions, windows or monitors");
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
    assert.equal(actions(mounted).at(-1).args.appName, "Firefox");
    assert.equal(actions(mounted).at(-1).args.iconPng, "bGl2ZS13aW5kb3ctaWNvbg==");
    await state.connect(state.runningConnections.find(r => r.sessionId === "remote"));
    assert.equal(actions(mounted).at(-1).args.sessionId, "remote");
    assert.equal(actions(mounted).at(-1).args.appName, "Firefox");
    assert.equal(actions(mounted).at(-1).args.iconPng, "Y2FjaGVkLWFwcC1pY29u");
    await state.connect(state.runningConnections.find(r => r.appId === "window-new"));
    assert.equal(actions(mounted).at(-1).command, "launch_app");
    assert.equal(actions(mounted).at(-1).args.appId, "window-new");
    assert.equal(actions(mounted).at(-1).args.appName, "Firefox");
    assert.equal(actions(mounted).at(-1).args.iconPng, "bGl2ZS13aW5kb3ctaWNvbg==");
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

test("launching a cached application forwards its Ubuntu icon without scanning again", async () => {
  const saved = storage();
  const first = mount(saved);
  await settled(first.state);
  first.unmount();
  const mounted = mount(saved);
  try {
    await settled(mounted.state);
    await mounted.state.launch(mounted.state.apps[0]);
    assert.equal(actions(mounted).at(-1).args.appName, "Firefox");
    assert.equal(actions(mounted).at(-1).args.iconPng, "Y2FjaGVkLWFwcC1pY29u");
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 0);
  } finally { mounted.unmount(); }
});

test("ordinary startup ignores retired scaling preferences and Mac display density", async () => {
  for (const oldScale of ["auto", "2", "1"]) {
    const saved = storage();
    saved.setItem("lan-remoteapp.connection", JSON.stringify({endpoint, renderScale:oldScale, applicationScene:true}));
    const mounted = mount(saved, { devicePixelRatio:2,
      launch_app: () => { throw {message:"connection",retrySameRequest:true}; },
    });
    try {
      await settled(mounted.state);
      assert.equal(mounted.state.applicationScene, true, "migration preserves the remaining display preference");
      assert.equal(JSON.parse(saved.getItem("lan-remoteapp.connection")).renderScale, undefined);
      await mounted.state.launch(app);
      const retina = actions(mounted).at(-1).args;
      assert.equal(retina.renderScale, 1);
      globalThis.window.devicePixelRatio = 1;
      await mounted.state.launch(app);
      assert.equal(actions(mounted).at(-1).args.renderScale, 1);
      assert.equal(actions(mounted).at(-1).args.requestId, retina.requestId,
        "changing Mac density must not start a second request");
    } finally { mounted.unmount(); }
  }
});

test("optional scene canvas is bounded, retained on retries, and switches a detached external session", async () => {
  const mounted = mount(storage(), { devicePixelRatio: 2,
    list_sessions: () => [{...session("old", "wayland-root"), mode:"app"}],
    list_desktop_windows: () => ({windows:[{...window("wayland-root", "old"), application_scene:true}], unavailable_reason:null}),
    launch_app: () => { throw {message:"connection", retrySameRequest:true}; },
  });
  try {
    await settled(mounted.state);
    assert.equal(mounted.state.applicationScene, false);
    mounted.state.applicationScene = true;
    await mounted.state.connect(mounted.state.runningConnections[0]);
    const sent = actions(mounted).at(-1);
    assert.equal(sent.command, "launch_app");
    assert.deepEqual(sent.args.scene, {width:1470,height:797});
    await mounted.state.connect(mounted.state.runningConnections[0]);
    assert.equal(actions(mounted).at(-1).args.requestId, sent.args.requestId);
    globalThis.window.screen = {availWidth:7680,availHeight:4320};
    await mounted.state.connect(mounted.state.runningConnections[0]);
    const large = actions(mounted).at(-1).args;
    assert.ok(large.scene.width <= 4096 && large.scene.height <= 4096);
    assert.ok(large.scene.width * large.scene.height <= 8_000_000);
    assert.notEqual(large.requestId, sent.args.requestId);
  } finally { mounted.unmount(); }
});

test("30/60 frame preference persists and reaches launch, resume and whole desktop without duplicating requests", async () => {
  const saved = storage();
  saved.setItem("lan-remoteapp.connection", JSON.stringify({endpoint, videoFPS:30}));
  let mounted = mount(saved, {
    launch_app: () => { throw {message:"connection", retrySameRequest:true}; },
    start_desktop: () => ({session_id:"screen",display:"0",pid:123,message:"started"}),
  });
  try {
    await settled(mounted.state);
    assert.equal(mounted.state.videoFPS, 30);
    await mounted.state.launch(app);
    const first = actions(mounted).at(-1).args;
    assert.equal(first.videoFps, 30);
    mounted.state.videoFPS = 60; mounted.state.save();
    await mounted.state.launch(app);
    assert.equal(actions(mounted).at(-1).args.videoFps, 60);
    assert.equal(actions(mounted).at(-1).args.requestId, first.requestId,
      "frame selection is a receiver option, not a second application launch");
    await mounted.state.resume("remote", "Firefox");
    assert.equal(actions(mounted).at(-1).args.videoFps, 60);
    await mounted.state.openDesktop({id:"DP-1",name:"Monitor",width:1920,height:1080,primary:true});
    assert.equal(mounted.calls.filter(c => c.command === "start_desktop").at(-1).args.videoFps, 60);
    mounted.state.videoFPS = 30; mounted.state.save();
  } finally { mounted.unmount(); }
  mounted = mount(saved);
  try { await settled(mounted.state); assert.equal(mounted.state.videoFPS, 30); }
  finally { mounted.unmount(); }
  for (const invalid of [undefined, 0, 90, "30", false]) {
    const old = storage();
    old.setItem("lan-remoteapp.connection", JSON.stringify({endpoint, videoFPS:invalid}));
    mounted = mount(old);
    try { await settled(mounted.state); assert.equal(mounted.state.videoFPS, 60); }
    finally { mounted.unmount(); }
  }
});

test("scene preference follows source capabilities while compatibility sessions resume normally", async () => {
  const supported = {...window("native-source"), application_scene:true};
  const compatibility = {...window("compat-source", "compat"), application_scene:false};
  const legacy = window("legacy-source");
  const mounted = mount(storage(), {
    list_sessions: () => [{...session("compat", "compat-source"), mode:"app"}],
    list_desktop_windows: () => ({windows:[supported, compatibility, legacy], unavailable_reason:null}),
  });
  try {
    await settled(mounted.state);
    mounted.state.applicationScene = true;
    await mounted.state.connect(mounted.state.runningConnections.find(row => row.appId === supported.id));
    assert.ok(actions(mounted).at(-1).args.scene, "an explicitly supported live source keeps the scene preference");
    await mounted.state.connect(mounted.state.runningConnections.find(row => row.appId === compatibility.id));
    assert.equal(actions(mounted).at(-1).command, "resume_session", "the preference must not switch an incompatible session");
    await mounted.state.connect(mounted.state.runningConnections.find(row => row.appId === legacy.id));
    assert.equal(actions(mounted).at(-1).args.scene, null, "a legacy publisher has not advertised scene support");
    assert.match(mounted.state.notice, /兼容窗口显示/);
    assert.equal(mounted.state.applicationScene, true, "compatibility does not disable the preference for other apps");
    await mounted.state.launch({...compatibility, id:"untouched-compat"});
    assert.equal(actions(mounted).at(-1).args.scene, null);
    mounted.state.applicationScene = false;
    await mounted.state.launch(supported);
    assert.equal(actions(mounted).at(-1).args.scene, null);
  } finally { mounted.unmount(); }
});

test("catalog entry uses only a unique verified live source for scene presentation", async () => {
  let windows = [{...window("native-root"), app_id:app.id, application_scene:true}];
  let remote = [];
  const mounted = mount(storage(), {
    list_sessions: () => remote,
    list_desktop_windows: () => ({windows, unavailable_reason:null}),
  });
  try {
    await settled(mounted.state);
    mounted.state.applicationScene = true;
    await mounted.state.launch(app);
    assert.ok(actions(mounted).at(-1).args.scene);
    const sceneRequest = actions(mounted).at(-1).args.requestId;
    windows = [{...window("compat-root"), app_id:app.id, application_scene:false}];
    await mounted.state.refresh();
    await mounted.state.launch(app);
    assert.equal(actions(mounted).at(-1).args.scene, null);
    assert.notEqual(actions(mounted).at(-1).args.requestId, sceneRequest);
    windows = [windows[0], {...window("other-root"), app_id:app.id, application_scene:true}];
    await mounted.state.refresh();
    await mounted.state.launch(app);
    assert.equal(actions(mounted).at(-1).args.scene, null, "multiple live windows must not be guessed from an app name");
    windows = [{...window("native-root"), app_id:app.id, application_scene:true}];
    remote = [{...session("private", app.id, 228), mode:"app"}];
    await mounted.state.refresh();
    await mounted.state.launch(app);
    assert.equal(actions(mounted).at(-1).args.scene, null, "an independent application session retains its presentation");
    windows = [];
    remote = [];
    await mounted.state.refresh();
    await mounted.state.launch(app);
    assert.equal(actions(mounted).at(-1).args.scene, null, "a new application has not advertised live scene support");
  } finally { mounted.unmount(); }
});

test("a slow launch shows starting, then becomes connectable without another launch", async () => {
  let windowCount = 0;
  let launched = false;
  const mounted = mount(storage(), {
    list_sessions: () => launched ? [{ ...session("slow", "firefox", 228), window_count: windowCount }] : [],
    launch_app: () => {
      launched = true;
      throw { message: "应用尚未产生窗口", retrySameRequest: true, code: "application_window_pending" };
    },
  });
  try {
    await settled(mounted.state);
    await mounted.state.launch(app);
    assert.equal(mounted.state.error, "");
    assert.match(mounted.state.notice, /Firefox启动中/);
    assert.equal(mounted.state.currentStarting.length, 1);
    assert.equal(mounted.state.runningConnections.length, 0);
    await mounted.state.launch(app);
    assert.equal(actions(mounted).filter(call => call.command === "launch_app").length, 1);
    windowCount = 1;
    await waitUntil(() => mounted.state.currentStarting.length === 0);
    assert.equal(mounted.state.currentStarting.length, 0);
    assert.equal(mounted.state.runningConnections[0].sessionId, "slow");
    assert.match(mounted.state.notice, /已出现窗口/);
    assert.ok(mounted.calls.filter(call => call.command === "list_sessions").length >= 3,
      "the launcher polled the remote session automatically");
    assert.equal(actions(mounted).filter(call => call.command === "launch_app").length, 1);
    await mounted.state.connect(mounted.state.runningConnections[0]);
    assert.equal(actions(mounted).at(-1).command, "resume_session");
  } finally { mounted.unmount(); }
});

test("a pending launch whose session exits stops claiming it is still starting", async () => {
  let present = true;
  const mounted = mount(storage(), {
    list_sessions: () => present ? [{ ...session("slow", "firefox", 228), window_count: 0 }] : [],
    launch_app: () => { throw { message: "等待窗口", retrySameRequest: true, code: "application_window_pending" }; },
  });
  try {
    await settled(mounted.state);
    await mounted.state.launch(app);
    present = false;
    await mounted.state.refresh();
    await mounted.state.refresh();
    assert.equal(mounted.state.currentStarting.length, 0);
    assert.match(mounted.state.error, /会话已结束/);
    assert.equal(mounted.state.notice, "");
    assert.equal(mounted.state.isStarting("firefox"), false);
  } finally { mounted.unmount(); }
});

test("a delayed window constraint failure stops startup polling without relaunching the app", async () => {
  let reason = null;
  const mounted = mount(storage(), {
    list_sessions: () => [{ ...session("slow", "firefox", 228), window_count: reason ? null : 0, unavailable_reason: reason }],
    launch_app: () => { throw { message: "等待窗口", retrySameRequest: true, code: "application_window_pending" }; },
  });
  try {
    await settled(mounted.state);
    await mounted.state.launch(app);
    assert.equal(mounted.state.currentStarting.length, 1);
    reason = "window_size_unavailable";
    await waitUntil(() => mounted.state.currentStarting.length === 0);
    assert.match(mounted.state.error, /尺寸约束/);
    assert.equal(mounted.state.notice, "");
    assert.equal(mounted.state.isStarting("firefox"), false);
    assert.equal(mounted.state.runningConnections.length, 0);
    assert.equal(actions(mounted).filter(call => call.command === "launch_app").length, 1);
  } finally { mounted.unmount(); }
});

test("whole desktop connects only the selected monitor and resumes its detached session", async () => {
  const monitors = [
    { id: "eDP-1", name: "内置屏幕", width: 2880, height: 1800, primary: true },
    { id: "HDMI-1", name: "外接屏幕", width: 1920, height: 1080, primary: false },
  ];
  let sessions = [];
  const mounted = mount(storage(), {
    list_desktop_monitors: () => ({ monitors, unavailable_reason: null }),
    list_sessions: () => sessions,
    start_desktop: ({ monitorId }) => {
      assert.equal(monitorId, "HDMI-1");
      sessions = [{ ...session("screen-session", "screen-HDMI-1"), mode: "desktop", monitor_id: monitorId, window_count: 1 }];
      return { session_id: "screen-session", display: "0", pid: 123, message: "已连接桌面" };
    },
  });
  try {
    await settled(mounted.state);
    assert.deepEqual(mounted.state.desktopMonitors.map(monitor => monitor.id), ["eDP-1", "HDMI-1"]);
    await mounted.state.openDesktop(monitors[1]);
    assert.equal(mounted.calls.filter(call => call.command === "start_desktop").length, 1);
    assert.equal(mounted.state.runningConnections.length, 0);
    await mounted.state.openDesktop(monitors[1]);
    assert.equal(mounted.calls.filter(call => call.command === "start_desktop").length, 1);
    assert.equal(mounted.calls.filter(call => call.command === "resume_session").at(-1).args.sessionId, "screen-session");
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
    state.endpoint = { ...endpoint, peerId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" };
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

test("network restoration recovers metadata without replaying app operations", async () => {
  let phase = "online";
  function requireConnection() {
    if (phase === "offline") throw { kind: "connection", message: "设备网络离线" };
    if (phase === "authentication") throw { kind: "pairing_required", message: "设备已解除配对，请重新配对" };
  }
  const mounted = mount(storage(), {
    list_sessions: () => { requireConnection(); return [session("remote", "firefox", 228)]; },
    list_desktop_windows: () => { requireConnection(); return { windows: [], unavailable_reason: null }; },
    resume_session: () => { requireConnection(); return { session_id: "remote", pid: 123, message: "客户端已启动" }; },
  });
  try {
    const { state } = mounted;
    await settled(state);
    phase = "offline";
    await state.refresh();
    assert.match(state.windowWarning, /设备网络离线/);
    assert.equal(state.apps[0].name, "Firefox");
    phase = "authentication";
    await state.refresh();
    assert.match(state.sessionWarning, /设备已解除配对/);
    assert.doesNotMatch(state.windowWarning, /object Object/);
    await state.connect(state.runningConnections[0]);
    assert.match(state.error, /设备已解除配对/);
    phase = "online";
    mounted.listeners.get("online")();
    await settled(state);
    assert.equal(state.error, "");
    assert.equal(state.sessionWarning, "");
    assert.equal(state.windowWarning, "");
    assert.equal(actions(mounted).length, 1, "online event must not replay the failed resume");
    assert.equal(state.runningConnections[0].sessionId, "remote");
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 1);
    assert.equal(actions(mounted).length, 1);
  } finally { mounted.unmount(); }
  assert.equal(mounted.listeners.size, 0);
});

test("a stuck previous connection cannot block network recovery", async () => {
  for (const recovery of ["online"]) {
    let stalled = false;
    const rejectOld = [];
    const mounted = mount(storage(), {
      list_sessions: () => stalled ? new Promise((_, reject) => rejectOld.push(reject)) : [session("remote", "firefox", 228)],
      list_desktop_windows: () => stalled ? new Promise((_, reject) => rejectOld.push(reject)) : { windows: [], unavailable_reason: null },
      });
    try {
      await settled(mounted.state);
      stalled = true;
      mounted.state.onFocus();
      assert.equal(rejectOld.length, 2);
      stalled = false;
      mounted.listeners.get(recovery)();
      await settled(mounted.state);
      assert.equal(mounted.calls.filter(c => c.command === "list_sessions").length, 3);
      for (const reject of rejectOld) reject({ kind: "connection", message: "old connection failed" });
      await settled(mounted.state);
      assert.equal(mounted.state.windowWarning, "");
      assert.equal(mounted.state.sessionWarning, "");
      assert.equal(actions(mounted).length, 0);
    } finally {
      for (const reject of rejectOld) reject(new Error("test cleanup"));
      mounted.unmount();
    }
  }
});

test("metadata reconnects with bounded backoff without clicks, catalog scans or repeated app operations", async () => {
  const originalSet = globalThis.setTimeout, originalClear = globalThis.clearTimeout;
  const timers = new Map(); let sequence = 0, offline = false, auth = false;
  globalThis.setTimeout = (callback, delay) => { const id = ++sequence; timers.set(id, {callback, delay}); return id; };
  globalThis.clearTimeout = id => timers.delete(id);
  function requireConnection() {
    if (auth) throw { kind: "pairing_required", message: "设备授权已失效" };
    if (offline) throw { kind: "connection", message: "network offline" };
  }
  const mounted = mount(storage(), {
    list_sessions: () => { requireConnection(); return [session("remote", "firefox", 228)]; },
    list_desktop_windows: () => { requireConnection(); return { windows: [], unavailable_reason: null }; },
  });
  try {
    await settled(mounted.state);
    offline = true; await mounted.state.refresh();
    for (const delay of [1000, 2000, 4000, 8000, 15000, 15000]) {
      assert.equal(timers.size, 1);
      const [id, timer] = [...timers][0]; timers.delete(id);
      assert.equal(timer.delay, delay); timer.callback(); await settled(mounted.state);
    }
    assert.match(mounted.state.notice, /自动重连/);
    offline = false;
    const [id, timer] = [...timers][0]; timers.delete(id); timer.callback(); await settled(mounted.state);
    assert.equal(timers.size, 0);
    assert.equal(mounted.state.windowWarning, "");
    assert.match(mounted.state.notice, /自动恢复/);
    assert.equal(actions(mounted).length, 0);
    assert.equal(mounted.calls.filter(c => c.command === "list_apps").length, 1);
    auth = true; await mounted.state.refresh();
    assert.equal(timers.size, 0, "revoked pairing must stop automatic retries");
    auth = false; offline = true; await mounted.state.refresh();
    assert.equal(timers.size, 1);
    mounted.state.endpoint = { ...endpoint, peerId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" }; await nextTick();
    assert.equal(timers.size, 0, "changing endpoint must cancel recovery of the previous device");
  } finally {
    mounted.unmount(); globalThis.setTimeout = originalSet; globalThis.clearTimeout = originalClear;
  }
});

test("legacy connection settings cannot become a paired device or trigger connection", async () => {
  const saved = storage();
  saved.setItem("lan-remoteapp.connection", JSON.stringify({endpoint:{host:"ubuntu",user:"alice",port:22},renderScale:"2"}));
  saved.setItem("lan-remoteapp.devices", JSON.stringify([{host:"ubuntu",user:"alice",port:22}]));
  const mounted = mount(saved);
  try {
    await settled(mounted.state);
    assert.equal(mounted.state.valid, false);
    assert.equal(mounted.state.renderScale, undefined);
    assert.equal(saved.getItem("lan-remoteapp.devices"), null);
    assert.doesNotMatch(saved.getItem("lan-remoteapp.connection"), /host|user|port/);
    assert.equal(mounted.calls.filter(c => ["list_apps","list_sessions","list_desktop_windows"].includes(c.command)).length, 0);
  } finally { mounted.unmount(); }
});

test("menu navigation preserves the active device, app search, display preference and live sessions", async () => {
  const peer = { device_id: endpoint.peerId, name: "Ubuntu", address: "10.0.1.66:42424", fingerprint: "test", connected: true };
  const mounted = mount(storage(), {
    network_action: () => ({ peers: [peer] }),
    local_node_info: () => ({ name: "Mac", sharing_enabled: true, permissions: { screen_recording: true, accessibility: true } }),
  });
  try {
    const { state } = mounted;
    await settled(state);
    state.search = "fire";
    state.applicationScene = true;
    state.pairAddress = "192.168.1.20:42424";
    const calls = mounted.calls.length;
    for (const page of ["devices", "sharing", "display", "history", "apps"]) {
      state.activePage = page;
      await nextTick();
      assert.equal(state.currentPage.id, page);
      assert.equal(state.selectedPeer.device_id, endpoint.peerId);
      assert.equal(state.filtered[0].name, "Firefox");
      assert.equal(state.applicationScene, true);
      assert.equal(state.pairAddress, "192.168.1.20:42424");
      assert.equal(state.runningConnections[0].sessionId, "remote");
    }
    assert.equal(mounted.calls.length, calls, "changing pages must not reconnect, rescan apps or change sharing");
    await state.launch(state.installed[0]);
    assert.equal(actions(mounted).at(-1).args.renderScale, 1);
    assert.equal(actions(mounted).at(-1).args.endpoint.peerId, endpoint.peerId);
  } finally { mounted.unmount(); }
});

test("device actions return to the workspace and keep catalogs and connection records associated with each peer", async () => {
  const peers = [
    { device_id: endpoint.peerId, name: "Ubuntu", address: "10.0.1.66:42424", fingerprint: "first", connected: true },
    { device_id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", name: "Ubuntu", address: "10.0.1.67:42424", fingerprint: "second", connected: false },
  ];
  const mounted = mount(storage(), {
    network_action: () => ({ peers }),
    list_apps: ({ endpoint: target }) => ({ apps: [{ ...app, name: target.peerId === peers[0].device_id ? "Firefox" : "Files" }], discovery_error: null }),
    list_sessions: ({ endpoint: target }) => [session(target.peerId, "firefox", 228)],
  });
  try {
    const { state } = mounted;
    await settled(state);
    await state.launch(state.apps[0]);
    state.activePage = "devices";
    await state.selectPeer(peers[1]);
    assert.equal(state.activePage, "apps");
    assert.equal(state.selectedPeer.address, peers[1].address, "same display names must not alias devices");
    assert.equal(state.apps[0].name, "Files");
    assert.equal(state.runningConnections[0].sessionId, peers[1].device_id);
    await state.launch(state.apps[0]);
    assert.equal(actions(mounted).at(-1).args.endpoint.peerId, peers[1].device_id);
    assert.deepEqual(state.launches.map(entry => entry.peerId), [peers[1].device_id, peers[0].device_id]);
    await state.changePeer({ target: { value: peers[0].device_id } });
    assert.equal(state.apps[0].name, "Firefox");
    assert.equal(state.runningConnections[0].sessionId, peers[0].device_id);
    assert.equal(mounted.calls.filter(call => call.command === "list_apps").length, 2, "returning to a device reuses its own catalog");
    assert.equal(state.launches.length, 2, "switching devices preserves the session's connection history");
  } finally { mounted.unmount(); }
});

test("a late sharing-status poll cannot undo a completed stop-sharing result", async () => {
  const node = { device_id: "local", name: "Mac", sharing_enabled: true,
    permissions: { screen_recording: true, accessibility: true } };
  let resolvePoll;
  let delayed = false;
  const mounted = mount(storage(), {
    local_node_info: () => delayed ? new Promise(resolve => { resolvePoll = resolve; }) : node,
    set_local_sharing: ({ enabled }) => ({ ...node, sharing_enabled: enabled }),
  });
  try {
    const { state } = mounted;
    await settled(state);
    delayed = true;
    const oldPoll = state.readLocal();
    await state.toggleSharing();
    assert.equal(state.localNode.sharing_enabled, false);
    resolvePoll(node);
    await oldPoll;
    assert.equal(state.localNode.sharing_enabled, false, "stale read must not say sharing is enabled after stopping it");
    await state.toggleSharing();
    assert.equal(mounted.calls.filter(call => call.command === "set_local_sharing").at(-1).args.enabled, true,
      "the next click must enable sharing, not send a second stop based on stale state");
  } finally { mounted.unmount(); }
});

test("sharing mutation excludes concurrent polls and duplicate clicks until confirmed", async () => {
  const node = { device_id: "local", name: "Mac", sharing_enabled: true,
    permissions: { screen_recording: true, accessibility: true } };
  let finish;
  const mounted = mount(storage(), {
    local_node_info: () => node,
    set_local_sharing: () => new Promise(resolve => { finish = resolve; }),
  });
  try {
    const { state } = mounted;
    await settled(state);
    const reads = mounted.calls.filter(call => call.command === "local_node_info").length;
    const stopping = state.toggleSharing();
    await state.readLocal();
    void state.toggleSharing();
    assert.equal(mounted.calls.filter(call => call.command === "local_node_info").length, reads);
    assert.equal(mounted.calls.filter(call => call.command === "set_local_sharing").length, 1);
    finish({ ...node, sharing_enabled: false });
    await stopping;
    assert.equal(state.localBusy, false);
    assert.equal(state.localNode.sharing_enabled, false);
  } finally { mounted.unmount(); }
});

test("application-scoped automatic rules allow two all rules and propagate the stable identity on manual connect", async () => {
  const status = {rules: [], connectedApplications: ["firefox", "idea"], warning: ""};
  const mounted = mount(storage(), {
    list_desktop_windows: () => ({windows: [{...window("window-live"), application_key:"firefox"}], unavailable_reason:null}),
    automatic_window_status: () => structuredClone(status),
    set_automatic_window_rule: ({preference}) => {
      status.rules = status.rules.filter(p => p.applicationKey !== preference.applicationKey);
      status.rules.push(preference);
      return structuredClone(status);
    },
  });
  try {
    await settled(mounted.state);
    const state = mounted.state;
    state.apps.push({id:"idea", name:"IDEA", source:"desktop", description:""});
    state.automaticApp = "firefox";
    await state.saveAutomatic({mode:"all", applications:[]});
    state.automaticApp = "idea";
    await state.saveAutomatic({mode:"all", applications:[]});
    assert.deepEqual(status.rules.map(p => [p.applicationKey, p.rule.mode]), [["firefox","all"],["idea","all"]]);
    assert.ok(status.rules.every(p => p.peerId === endpoint.peerId));
    await state.connect(state.runningConnections.find(row => row.appId === "window-live"));
    assert.equal(actions(mounted).at(-1).args.applicationKey, "firefox");
    assert.equal(actions(mounted).at(-1).args.appId, "window-live");
  } finally { mounted.unmount(); }
});

test("obsolete automatic sources cannot be enabled and their saved rule can still be disabled", async () => {
  const rule = {peerId:endpoint.peerId, applicationKey:"old", name:"Firefox（现有窗口）", rule:{mode:"all", applications:[]}};
  const mounted = mount(storage(), {
    list_apps: () => ({apps:[app, {...app, id:"old", name:rule.name, unavailable_reason:"原进程已退出"}], discovery_error:null}),
    automatic_window_status: () => ({rules:[rule], connectedApplications:["firefox"], warning:""}),
    set_automatic_window_rule: ({preference}) => ({rules:[preference], connectedApplications:["firefox"], warning:""}),
  });
  try {
    await settled(mounted.state);
    const state = mounted.state;
    await state.readAutomatic();
    assert.equal(state.automaticKey, "firefox");
    state.automaticApp = "old";
    assert.equal(state.automaticUnavailable, "原进程已退出");
    await state.saveAutomatic({mode:"all", applications:[]});
    assert.equal(mounted.calls.filter(c => c.command === "set_automatic_window_rule").length, 0);
    await state.saveAutomatic({mode:"off", applications:[]});
    assert.equal(mounted.calls.find(c => c.command === "set_automatic_window_rule").args.preference.rule.mode, "off");
  } finally { mounted.unmount(); }
});

test("an automatic-rule save finishing after device selection cannot overwrite the new device settings", async () => {
  let finish;
  const mounted = mount(storage(), {
    automatic_window_status: () => ({rules: [], connectedApplications: [], warning:""}),
    set_automatic_window_rule: ({preference}) => new Promise(resolve => { finish = () => resolve({rules:[preference], connectedApplications:[], warning:""}); }),
  });
  try {
    await settled(mounted.state);
    const saving = mounted.state.saveAutomatic({mode:"all", applications:[]});
    mounted.state.endpoint = {peerId:"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"};
    await nextTick();
    finish(); await saving;
    assert.equal(mounted.state.automaticStatus.rules.length, 0);
    assert.equal(mounted.state.automaticSaving, false);
  } finally { mounted.unmount(); }
});

test("device takeover works with no app catalogue or connected anchor and preserves app rules", async () => {
  const rule = {peerId:endpoint.peerId, applicationKey:"idea", name:"IDEA", rule:{mode:"selected", applications:["chooser"]}};
  let status = {rules:[rule], connectedApplications:[], warning:"", takeoverEnabled:false, managedWindows:0};
  const mounted = mount(storage(), {
    list_apps: () => ({apps:[], discovery_error:null}),
    list_sessions: () => [],
    automatic_window_status: () => structuredClone(status),
    set_device_window_takeover: args => { status.takeoverEnabled=args.enabled; return structuredClone(status); },
    disconnect_device_windows: () => { status.takeoverEnabled=false; return structuredClone(status); },
  });
  try {
    await settled(mounted.state);
    const state=mounted.state;
    state.videoFPS=30;
    await state.deviceWindowAction();
    assert.equal(state.automaticStatus.takeoverEnabled, true);
    assert.deepEqual(mounted.calls.find(c=>c.command==='set_device_window_takeover').args, {endpoint,enabled:true,videoFps:30});
    await state.saveAutomatic({mode:"off",applications:[]});
    assert.equal(mounted.calls.filter(c=>c.command==='set_automatic_window_rule').length,0);
    await state.deviceWindowAction();
    assert.equal(state.automaticStatus.takeoverEnabled, false);
    assert.deepEqual(state.automaticStatus.rules,[rule]);
    await state.deviceWindowAction(true);
    assert.deepEqual(mounted.calls.find(c=>c.command==='disconnect_device_windows').args,{endpoint});
  } finally {mounted.unmount();}
});

test("device takeover handles failed saves and stale responses after changing device", async () => {
  let finish, fail=true;
  const mounted=mount(storage(), {
    automatic_window_status:()=>({rules:[],connectedApplications:[],warning:'',takeoverEnabled:false,managedWindows:0}),
    set_device_window_takeover:()=>{if(fail) throw new Error('disk full');return new Promise(resolve=>{finish=()=>resolve({rules:[],connectedApplications:[],warning:'',takeoverEnabled:true,managedWindows:5});});},
  });
  try {
    await settled(mounted.state);
    await mounted.state.deviceWindowAction();
    assert.equal(mounted.state.automaticStatus.takeoverEnabled,false);
    assert.match(mounted.state.takeoverError,/保存失败/);
    fail=false;
    const saving=mounted.state.deviceWindowAction();
    mounted.state.endpoint={peerId:'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb'};
    await nextTick();finish();await saving;
    assert.equal(mounted.state.automaticStatus.takeoverEnabled,false);
    assert.equal(mounted.state.automaticSaving,false);
  } finally {mounted.unmount();}
});
