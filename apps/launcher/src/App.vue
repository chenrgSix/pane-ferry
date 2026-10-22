<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { PendingLaunches } from "./session-store";
import { launchFailure } from "./launch-failure";
import { CatalogCache, readCatalog } from "./catalog-cache";
import { endpointKey, validPeerId } from "./launcher-types";
import { connections } from "./connections";
import { isConnectionFailure, requestFailure } from "./request-failure";
import type { Connection } from "./connections";
import type { AppCatalog, AppInfo, DesktopWindow, DesktopWindows, Endpoint, LaunchTarget, SessionInfo } from "./launcher-types";
import AppIcon from "./AppIcon.vue";
import { retireLegacyPreferences } from "./connection-preferences";
type Result = {
  session_id: string | null;
  display: string | null;
  pid: number;
  message: string;
};
const endpoint = ref<Endpoint>({ peerId: "" }),
  apps = ref<AppInfo[]>([]),
  desktopWindows = ref<DesktopWindow[]>([]),
  discoveryWarning = ref(""),
  windowWarning = ref(""),
  sessionWarning = ref(""),
  search = ref("");
const busy = ref(false),
  renderScale = ref("auto"),
  error = ref(""),
  notice = ref(""),
  catalogSavedAt = ref<number | null>(null),
  catalogPersisted = ref(false),
  connected = ref(""),
  runtimeEndpoint = ref(""),
  attemptedEndpoint = ref(""),
  actionConnectionError = ref(false),
  sessions = ref<SessionInfo[]>([]),
  launches = ref<{ name: string; result: Result }[]>([]);
let storage: Storage | undefined;
try {
  storage = localStorage;
} catch {}
retireLegacyPreferences(storage);
const attempts = new PendingLaunches(storage);
type LocalNode = {device_id:string; name:string; sharing_enabled:boolean; permissions:{screen_recording:boolean; accessibility:boolean}};
const localNode = ref<LocalNode|null>(null), localWarning=ref(""), localBusy=ref(false);
type NativePeer = {device_id:string; name:string; address:string; fingerprint:string; connected:boolean};
const nativePeers = ref<NativePeer[]>([]), nativeWarning=ref(""), pairAddress=ref(""), pairCode=ref(""), ownInvite=ref(""), pairing=ref(false);
const addressEdits=ref<Record<string,string>>({});
let inviteTimer:ReturnType<typeof setTimeout>|undefined;
async function networkAction(body:Record<string,unknown>) { return invoke<any>("network_action",{body}); }
async function readNetwork() {
  try { const status=await networkAction({op:"status"}); nativePeers.value=status.peers;
    for(const peer of nativePeers.value) addressEdits.value[peer.device_id] ??= peer.address;
    nativeWarning.value=""; }
  catch { nativeWarning.value="直接连接组件尚未就绪。"; }
}
async function inviteDevice() {
  pairing.value=true;
  try { ownInvite.value=(await networkAction({op:"invite"})).invitation;
    if(inviteTimer) clearTimeout(inviteTimer); inviteTimer=setTimeout(()=>ownInvite.value="",300000);
  } catch { nativeWarning.value="邀请生成失败，请重试。"; } finally {pairing.value=false;}
}
async function pairDevice() {
  pairing.value=true;
  try { await networkAction({op:"pair",address:pairAddress.value.trim(),invitation:pairCode.value.trim()}); pairCode.value=""; await readNetwork(); notice.value="设备已双向配对，可选择设备使用应用。"; }
  catch {nativeWarning.value="配对未完成，请检查设备 IP、端口和五分钟内生成的邀请。身份变更需先撤销旧配对。";}
  finally {pairing.value=false;}
}
async function selectPeer(peer:NativePeer) {
  endpoint.value={peerId:peer.device_id};
  await nextTick(); await load();
}
async function updatePeerAddress(peer:NativePeer) {
  pairing.value=true;
  try {await networkAction({op:"address",device_id:peer.device_id,address:addressEdits.value[peer.device_id] || peer.address}); await readNetwork();}
  catch {nativeWarning.value="地址更新失败，请填写 IP:端口。";} finally {pairing.value=false;}
}
async function revokePeer(peer:NativePeer) {
  pairing.value=true;
  try { await networkAction({op:"revoke",device_id:peer.device_id}); await readNetwork();
    if(endpoint.value.peerId===peer.device_id) endpoint.value={peerId:""};
  } catch {nativeWarning.value="撤销未完成，请重试。";} finally {pairing.value=false;}
}
async function readLocal() {
  try {localNode.value=await invoke<LocalNode>("local_node_info"); localWarning.value="";} catch {localWarning.value="本机共享组件尚未就绪。";}
}
async function toggleSharing() {
  localBusy.value=true;
  try {localNode.value=await invoke<LocalNode>("set_local_sharing",{enabled:!localNode.value?.sharing_enabled});
  } catch {localWarning.value="共享状态未能更新，请重试。";} finally {localBusy.value=false;}
}
async function requestPermissions() {
  localBusy.value=true;
  try {localNode.value=await invoke<LocalNode>("request_local_permissions");}
  catch {localWarning.value="请在 macOS 系统设置中允许 窗渡 PaneFerry 的录屏与辅助功能。";}
  finally {localBusy.value=false;}
}
async function showLocalComponent() {
  try {await invoke("reveal_local_component");}
  catch {localWarning.value="未能在访达显示本机共享组件。";}
}
let localTimer:ReturnType<typeof setInterval>|undefined;
const catalogCache = new CatalogCache(storage);
const filtered = computed(() =>
  apps.value.filter((a) =>
    `${a.name} ${a.id}`.toLowerCase().includes(search.value.toLowerCase()),
  ),
);
const installed = computed(() => filtered.value.filter(app => app.source === "desktop"));
const custom = computed(() => filtered.value.filter(app => app.source === "custom"));
const runningConnections = computed(() => connections(apps.value, desktopWindows.value, sessions.value));
const filteredConnections = computed(() => runningConnections.value.filter(row =>
  `${row.name} ${row.title}`.toLowerCase().includes(search.value.toLowerCase())));
const signature = () => endpointKey(endpoint.value),
  catalogCurrent = computed(() => connected.value === signature()),
  runtimeCurrent = computed(() => runtimeEndpoint.value === signature());
let runtimeRevision = 0;
let focusRead: Promise<void> | undefined;
let recoveryTimer: ReturnType<typeof setTimeout> | undefined;
let recoveryAttempt = 0;
let disposed = false;
function stopRetry(reset = true) {
  if (recoveryTimer !== undefined) clearTimeout(recoveryTimer);
  recoveryTimer = undefined;
  if (reset) recoveryAttempt = 0;
}
function retryRuntime() {
  if (disposed || recoveryTimer !== undefined) return;
  const delay = [1000, 2000, 4000, 8000, 15000][Math.min(recoveryAttempt++, 4)];
  recoveryTimer = setTimeout(() => { recoveryTimer = undefined; onFocus(); }, delay);
}
const valid = computed(() => validPeerId(endpoint.value.peerId));
function save() {
  try {
    storage?.setItem(
      "lan-remoteapp.connection",
      JSON.stringify({ endpoint: endpoint.value, renderScale: renderScale.value }),
    );
  } catch {}
}
try {
  const saved = JSON.parse(storage?.getItem("lan-remoteapp.connection") || "null");
  if (["auto", "1", "2"].includes(saved?.renderScale)) renderScale.value = saved.renderScale;
  if (validPeerId(saved?.endpoint?.peerId)) endpoint.value = { peerId: saved.endpoint.peerId };
} catch {}
function restoreCatalog() {
  stopRetry();
  const cached = catalogCache.get(signature());
  apps.value = cached?.apps ?? [];
  connected.value = cached ? signature() : "";
  catalogSavedAt.value = cached?.savedAt ?? null;
  catalogPersisted.value = catalogCache.isPersisted(signature());
  desktopWindows.value = [];
  sessions.value = [];
  runtimeEndpoint.value = "";
  attemptedEndpoint.value = cached ? signature() : "";
  actionConnectionError.value = false;
  runtimeRevision++;
  focusRead = undefined;
  discoveryWarning.value = "";
  windowWarning.value = "";
  sessionWarning.value = "";
  error.value = "";
  notice.value = "";
}
async function load(refreshApps = false) {
  if (!valid.value || busy.value) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  const sent = { ...endpoint.value };
  attemptedEndpoint.value = endpointKey(sent);
  try {
    const [catalog] = await Promise.allSettled([
      readCatalog(catalogCache, endpointKey(sent), () => invoke<AppCatalog>("list_apps", { endpoint: sent }), refreshApps),
      readRuntime(sent),
    ]);
    if (signature() !== endpointKey(sent)) return;
    const failures: string[] = [];
    if (catalog.status === "fulfilled") {
      apps.value = catalog.value.catalog.apps;
      discoveryWarning.value = catalog.value.catalog.discovery_error || "";
      catalogSavedAt.value = catalog.value.savedAt;
      catalogPersisted.value = catalog.value.persisted;
      connected.value = endpointKey(sent);
      notice.value = `${catalog.value.fromCache ? "已使用缓存" : "已读取"} ${apps.value.filter(app => app.source === "desktop").length} 个已安装应用。`;
    } else failures.push(`应用读取失败：${requestFailure(catalog.reason).message}`);
    actionConnectionError.value = catalog.status === "rejected" && isConnectionFailure(catalog.reason);
    error.value = failures.join(" ");
    save();
  } finally {
    busy.value = false;
  }
}
async function readRuntime(sent = { ...endpoint.value }) {
  const revision = ++runtimeRevision;
  const [running, desktop] = await Promise.allSettled([
    invoke<SessionInfo[]>("list_sessions", { endpoint: sent }),
    invoke<DesktopWindows>("list_desktop_windows", { endpoint: sent }),
  ]);
  // A focus refresh may overlap a connect/disconnect or a host change.
  if (disposed || signature() !== endpointKey(sent) || revision !== runtimeRevision) return;
  if (running.status === "fulfilled") {
    sessions.value = running.value;
    sessionWarning.value = "";
    runtimeEndpoint.value = endpointKey(sent);
  } else sessionWarning.value = `会话读取失败：${requestFailure(running.reason).message}`;
  if (desktop.status === "fulfilled") {
    desktopWindows.value = desktop.value.windows;
    windowWarning.value = desktop.value.unavailable_reason || "";
    runtimeEndpoint.value = endpointKey(sent);
  } else windowWarning.value = `运行窗口读取失败：${requestFailure(desktop.reason).message}`;
  if (running.status === "fulfilled" && desktop.status === "fulfilled") {
    const recovered = recoveryAttempt > 0;
    stopRetry();
    if (actionConnectionError.value) { error.value = ""; actionConnectionError.value = false; }
    if (recovered) notice.value = "连接已自动恢复。";
  } else {
    const failures = [running, desktop].filter(result => result.status === "rejected").map(result => requestFailure((result as PromiseRejectedResult).reason));
    if (failures.every(failure => failure.kind === "connection")) {
      notice.value = "连接暂时中断，正在自动重连…"; retryRuntime();
    } else stopRetry();
  }
}
async function refresh() {
  if (busy.value || !valid.value) return;
  busy.value = true;
  try {
    await readRuntime();
  } finally {
    busy.value = false;
  }
}
function record(name: string, result: Result) {
  launches.value.unshift({ name, result });
  launches.value = launches.value.slice(0, 20);
  notice.value = result.message;
  save();
}
async function launch(app: LaunchTarget) {
  if (busy.value || !valid.value || app.unavailable_reason) return;
  busy.value = true;
  error.value = "";
  actionConnectionError.value = false;
  notice.value = "";
  const sent = { ...endpoint.value };
  const key = `${signature()}:${app.id}:native:${renderScale.value}`;
  try {
    const requestId = attempts.id(key);
    const result = await invoke<Result>("launch_app", {
      endpoint: sent,
      appId: app.id,
      appName: app.name,
      requestId,
      renderScale: renderScale.value === "auto" ? null : Number(renderScale.value),
      iconPng: app.icon_png ?? null,
    });
    attempts.resolve(key);
    if (signature() === endpointKey(sent)) record(app.name, result);
  } catch (e) {
    const failure = launchFailure(e);
    if (!failure.retrySameRequest) attempts.resolve(key);
    if (signature() === endpointKey(sent)) {
      actionConnectionError.value = isConnectionFailure(e);
      error.value = failure.message + (failure.retrySameRequest
        ? " 可先刷新运行状态；再次点击会安全重试同一次启动。" : "");
    }
  } finally {
    await readRuntime(sent);
    busy.value = false;
  }
}
async function resume(sessionId: string, name: string, iconPng?: string | null) {
  if (busy.value || !valid.value) return;
  busy.value = true;
  error.value = "";
  actionConnectionError.value = false;
  notice.value = "";
  const sent = { ...endpoint.value };
  try {
    const result = await invoke<Result>("resume_session", {
      endpoint: sent,
      sessionId,
      appName: name,
      iconPng: iconPng ?? null,
    });
    if (signature() === endpointKey(sent)) record(name, result);
  } catch (e) {
    if (signature() === endpointKey(sent)) {
      error.value = requestFailure(e).message;
      actionConnectionError.value = isConnectionFailure(e);
    }
  } finally {
    await readRuntime(sent);
    busy.value = false;
  }
}
async function connect(row: Connection) {
  if (row.controlled) return;
  if (row.sessionId) await resume(row.sessionId, row.name, row.icon_png);
  else await launch({ id: row.appId, name: row.name, icon_png: row.icon_png });
}
function onFocus() {
  if (disposed || !valid.value || attemptedEndpoint.value !== signature() || focusRead) return;
  stopRetry(false);
  // Returning to the launcher must not disable or swallow the user's first click.
  const pending = readRuntime();
  focusRead = pending;
  void pending.finally(() => { if (focusRead === pending) focusRead = undefined; });
}
function onOnline() {
  // A request on the previous connection must not block recovery on a new one.
  runtimeRevision++;
  focusRead = undefined;
  onFocus();
}
watch(signature, restoreCatalog);
onMounted(() => {
  window.addEventListener("focus", onFocus);
  window.addEventListener("online", onOnline);
  restoreCatalog();
  if (valid.value) void load();
  void readLocal();
  void readNetwork();
  localTimer=setInterval(()=> {void readLocal(); void readNetwork();},5000);
});
onUnmounted(() => {
  disposed = true; runtimeRevision++; stopRetry();
  if(localTimer) clearInterval(localTimer);
  if(inviteTimer) clearTimeout(inviteTimer);
  window.removeEventListener("focus", onFocus);
  window.removeEventListener("online", onOnline);
});
</script>
<template>
  <div class="shell">
    <aside>
      <div class="brand">
        <span class="brand-icon">↗</span><span>窗渡<br /><b>PaneFerry</b></span>
      </div>
      <div class="nav">▦　应用工作台</div>
      <div class="aside-bottom">
        ● 本地网络 · 加密连接
        <p>应用留在原设备<br />窗口来到当前桌面</p>
        <small>开发预览 0.1.0</small>
      </div>
    </aside>
    <main>
      <header>
        <div>
          <div class="eyebrow">YOUR APPS, A DIFFERENT DESKTOP</div>
          <h1>让工作跨越桌面</h1>
          <p>共享本机应用，同时使用其他设备的应用。</p>
        </div>
        <span class="badge">QUIC 设备连接</span>
      </header>
      <section class="connection">
        <div class="section-title"><h2><span>本机应用共享</span></h2>
          <button :disabled="localBusy || !localNode" @click="toggleSharing">{{localNode?.sharing_enabled ? "停止共享" : "开启共享"}}</button></div>
        <p class="hint">{{localNode?.name || "Mac"}} · {{localNode?.sharing_enabled ? "已开启，已配对设备可读取本机应用" : "未开启"}}。本机应用继续显示在 Mac 桌面；收到的远程应用不会再次发布。</p>
        <p v-if="localNode && (!localNode.permissions.screen_recording || !localNode.permissions.accessibility)" class="hint">
          {{!localNode.permissions.screen_recording ? "需要录屏权限。" : ""}} {{!localNode.permissions.accessibility ? "需要辅助功能权限。" : ""}}
          <button :disabled="localBusy" @click="requestPermissions">允许共享所需权限</button>
          <button :disabled="localBusy" @click="showLocalComponent">显示共享组件</button>
          <br />系统列表已开启仍不生效时，点“显示共享组件”，移除权限列表中的旧条目，再添加访达选中的当前程序。
        </p>
        <p v-if="localWarning" class="alert error">{{localWarning}}</p>
      </section>
      <section class="connection">
        <div class="section-title"><h2><span>已配对设备</span></h2><button :disabled="pairing" @click="inviteDevice">生成本机配对邀请</button></div>
        <p class="hint">两端打开 窗渡 PaneFerry，用对方的局域网 IP 和邀请配对。邀请五分钟有效、仅可使用一次；请通过可信方式传递。</p>
        <textarea v-if="ownInvite" :value="ownInvite" readonly rows="3" aria-label="本机配对邀请" style="width:100%;overflow-wrap:anywhere" />
        <div class="fields"><label>对方地址<input v-model="pairAddress" placeholder="192.168.1.20:42424" :disabled="pairing" /></label>
          <label>对方邀请<input v-model="pairCode" type="password" autocomplete="off" :disabled="pairing" /></label>
          <button :disabled="pairing || !pairAddress || !pairCode" @click="pairDevice">配对设备</button></div>
        <div v-for="peer in nativePeers" :key="peer.device_id" class="desktop-window-row">
          <div class="window-label"><strong>{{peer.name}} · {{peer.address}}</strong><p :title="peer.fingerprint">指纹 {{peer.fingerprint}}</p></div>
          <span>{{peer.connected ? "已连接" : "已配对"}}</span>
          <input v-model="addressEdits[peer.device_id]" :disabled="pairing" aria-label="设备地址" style="width:150px" /><button :disabled="pairing" @click="updatePeerAddress(peer)">更新地址</button>
          <button :disabled="busy || pairing" @click="selectPeer(peer)">{{endpoint.peerId===peer.device_id ? "刷新应用" : "使用应用"}}</button>
          <button :disabled="pairing" @click="revokePeer(peer)">撤销配对</button>
        </div>
        <p v-if="nativeWarning" class="alert error">{{nativeWarning}}</p>
      </section>
      <section class="options">
        <div>
          <h2>01 <span>原生窗口</span></h2>
          <p class="hint">
            使用控制端输入法，支持主动复制与粘贴。新会话可按本机屏幕清晰度渲染。
          </p>
          <label for="render-scale">界面缩放（新会话）</label>
          <select id="render-scale" v-model="renderScale" :disabled="busy" @change="save">
            <option value="auto">自动 · 跟随本机像素密度</option>
            <option value="1">100%</option>
            <option value="2">200% · Retina</option>
          </select>
          <p class="hint">分辨率按本机窗口的实际像素尺寸调整；原生窗口的“显示”菜单可适应屏幕或输入像素宽高。已有会话保留原缩放。</p>
        </div>
      </section>
      <div v-if="error" class="alert error" role="alert">{{ error }}</div>
      <div v-if="notice" class="alert notice" role="status">{{ notice }}</div>
      <section class="catalog">
        <div class="section-title">
          <h2>
            02 <span>已安装应用</span><span class="count">{{ apps.filter(app => app.source === "desktop").length }}</span>
          </h2>
          <div class="catalog-actions"><input
            v-model="search"
            class="search"
            placeholder="搜索应用或窗口…"
            aria-label="搜索应用或窗口"
          /><button class="session-action" :disabled="!valid || busy" @click="load(true)">刷新应用</button></div>
        </div>
        <p v-if="catalogSavedAt" class="hint">上次更新：{{ new Date(catalogSavedAt).toLocaleString() }}。安装或卸载应用后可点“刷新应用”。</p>
        <p v-if="catalogSavedAt && !catalogPersisted" class="hint">本机缓存无法保存；当前列表仍可使用，退出启动器后需要重新读取。</p>
        <p v-if="discoveryWarning" class="alert error" role="status">{{ discoveryWarning }}</p>
        <div v-if="!apps.some(app => app.source === 'desktop')" class="empty">
          <div>▦</div>
          <h3>{{ busy ? "正在读取设备应用" : "选择设备后显示已安装应用" }}</h3>
          <p>
            首次连接后读取并缓存名称和图标，下次打开直接显示。
          </p>
        </div>
        <div v-else class="app-grid">
          <button
            v-for="app in installed"
            :key="app.id"
            class="app-card"
            :disabled="busy || !catalogCurrent || Boolean(app.unavailable_reason)"
            @click="launch(app)"
          >
            <AppIcon :icon="app.icon_png" :name="app.name" />
            <strong>{{ app.name }}</strong>
            <p>{{ app.unavailable_reason || app.description }}</p>
            <span class="launch-label">{{ app.unavailable_reason ? "暂不可启动" : "打开应用 ↗" }}</span>
          </button>
        </div>
        <p v-if="apps.some(app => app.source === 'desktop') && !installed.length" class="hint">没有匹配的应用。</p>
        <p v-if="apps.length && !catalogCurrent" class="hint">
          主机信息已改变，请连接当前主机。
        </p>
      </section>
      <section v-if="catalogCurrent || runtimeCurrent" class="catalog desktop-windows">
        <div class="section-title">
          <h2>03 <span>运行中的窗口</span><span class="count">{{ runningConnections.length }}</span></h2>
          <button class="session-action" :disabled="busy" @click="refresh">刷新运行状态</button>
        </div>
        <p class="hint">点击“连接”自动恢复已有会话或接管桌面窗口，保留应用内容。首次接管可能需要在 Ubuntu 确认授权；关闭本机窗口只会断开连接。</p>
        <p v-if="windowWarning" class="alert error" role="status">{{ windowWarning }}</p>
        <p v-if="sessionWarning" class="alert error" role="status">{{ sessionWarning }}</p>
        <p v-if="!runningConnections.length && !windowWarning && !sessionWarning" class="hint">{{ busy ? "正在读取运行状态…" : "当前没有运行中的窗口，打开应用后会显示在这里。" }}</p>
        <p v-else-if="runningConnections.length && !filteredConnections.length" class="hint">没有匹配的窗口。</p>
        <div v-for="row in filteredConnections" :key="row.key" class="desktop-window-row">
          <AppIcon :icon="row.icon_png" :name="row.name" />
          <div class="window-label"><strong>{{ row.name }}</strong><p :title="row.title">{{ row.title }}</p></div>
          <span class="window-state">{{ row.kind === "desktop" ? "Ubuntu 桌面" : "远程应用" }}{{ row.minimized ? " · 已最小化" : "" }}</span>
          <button class="session-action" :disabled="busy || row.controlled" @click="connect(row)">{{ row.controlled ? "已连接" : "连接" }}</button>
        </div>
      </section>
      <section v-if="custom.length" class="catalog">
        <div class="section-title"><h2><span>自定义入口</span><span class="count">{{ custom.length }}</span></h2></div>
        <p class="hint">保留 Ubuntu 上单独配置的启动参数和固定窗口入口。</p>
        <div class="app-grid">
          <button v-for="app in custom" :key="app.id" class="app-card" :disabled="busy || !catalogCurrent || Boolean(app.unavailable_reason)" @click="launch(app)">
            <AppIcon :icon="app.icon_png" :name="app.name" /><strong>{{ app.name }}</strong>
            <p>{{ app.unavailable_reason || app.description || app.id }}</p><span class="launch-label">{{ app.unavailable_reason ? "入口不可用" : "打开 ↗" }}</span>
          </button>
        </div>
      </section>
      <section v-if="launches.length" class="history">
        <h2>本次连接记录</h2>
        <div v-for="(entry, i) in launches" :key="i">
          <b>{{ entry.name }}</b
          ><span>原生窗口 · 客户端已启动</span
          ><code v-if="entry.result.session_id"
            >会话 {{ entry.result.session_id }}</code
          >
        </div>
        <p class="hint">请以实际窗口确认连接成功。</p>
      </section>
      <footer>
        一套软件 · 双向应用共享
        <span>剪贴板按需传输 · 不做后台同步</span>
      </footer>
    </main>
  </div>
</template>
