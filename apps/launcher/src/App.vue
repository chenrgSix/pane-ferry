<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { PendingLaunches } from "./session-store";
import { launchFailure } from "./launch-failure";
import { CatalogCache, readCatalog } from "./catalog-cache";
import { endpointKey } from "./launcher-types";
import { connections } from "./connections";
import { isConnectionFailure, requestFailure } from "./request-failure";
import type { Connection } from "./connections";
import type { AppCatalog, AppInfo, DesktopWindow, DesktopWindows, Endpoint, LaunchTarget, SessionInfo } from "./launcher-types";
import AppIcon from "./AppIcon.vue";
type Result = {
  session_id: string | null;
  display: string | null;
  pid: number;
  message: string;
};
const endpoint = ref<Endpoint>({ host: "", user: "", port: 22 }),
  password = ref(""),
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
  waitingForLogin = ref(false),
  loginNeeded = ref(false),
  sessions = ref<SessionInfo[]>([]),
  launches = ref<{ name: string; result: Result }[]>([]);
let storage: Storage | undefined;
try {
  storage = localStorage;
} catch {}
const attempts = new PendingLaunches(storage);
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
const valid = computed(() =>
  Boolean(
    endpoint.value.host.trim() &&
      endpoint.value.user.trim() &&
      Number.isInteger(endpoint.value.port) &&
      endpoint.value.port > 0 &&
      endpoint.value.port < 65536,
  ),
);
function save() {
  try {
    storage?.setItem(
      "lan-remoteapp.connection",
      JSON.stringify({ endpoint: endpoint.value, renderScale: renderScale.value }),
    );
  } catch {}
}
try {
  const s = JSON.parse(storage?.getItem("lan-remoteapp.connection") || "null");
  if (["auto", "1", "2"].includes(s?.renderScale)) renderScale.value = s.renderScale;
  if (
    s &&
    typeof s.endpoint?.host === "string" &&
    typeof s.endpoint?.user === "string" &&
    Number.isInteger(s.endpoint?.port)
  ) {
    endpoint.value = {
      host: s.endpoint.host,
      user: s.endpoint.user,
      port: s.endpoint.port,
    };
  }
} catch {}
function restoreCatalog() {
  stopRetry();
  password.value = "";
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
  waitingForLogin.value = false;
  loginNeeded.value = false;
  runtimeRevision++;
  focusRead = undefined;
  discoveryWarning.value = "";
  windowWarning.value = "";
  sessionWarning.value = "";
  error.value = "";
  notice.value = "";
}
async function connectHost() {
  if (busy.value || !valid.value) return;
  if (password.value) {
    busy.value = true;
    const sent = { ...endpoint.value };
    try {
      await invoke<void>("save_ssh_password", { endpoint: sent, password: password.value });
      password.value = "";
    } catch {
      error.value = "连接配置未能保存，请重试。";
      return;
    } finally { busy.value = false; }
  }
  await load();
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
    if (catalog.status === "rejected" && requestFailure(catalog.reason).kind === "authentication_required") loginNeeded.value = true;
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
    stopRetry(); loginNeeded.value = false;
    if (actionConnectionError.value) { error.value = ""; actionConnectionError.value = false; }
    if (waitingForLogin.value) { notice.value = "SSH 登录已恢复，窗口与会话列表已更新。"; waitingForLogin.value = false; }
    else if (recovered) notice.value = "连接已自动恢复。";
  } else {
    const failures = [running, desktop].filter(result => result.status === "rejected").map(result => requestFailure((result as PromiseRejectedResult).reason));
    loginNeeded.value = failures.some(failure => failure.kind === "authentication_required" || failure.kind === "host_verification");
    if (failures.every(failure => ["connection", "agent_unavailable"].includes(failure.kind))) {
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
async function login() {
  if (busy.value || !valid.value) return;
  busy.value = true;
  error.value = "";
  actionConnectionError.value = false;
  runtimeRevision++;
  focusRead = undefined;
  const sent = { ...endpoint.value };
  attemptedEndpoint.value = endpointKey(sent);
  try {
    await invoke<void>("open_ssh_login", { endpoint: sent });
    if (signature() !== endpointKey(sent)) return;
    waitingForLogin.value = true;
    notice.value = "已在系统终端打开 SSH 登录；完成认证后返回这里，会自动恢复窗口与会话列表。";
    save();
  } catch (reason) {
    if (signature() === endpointKey(sent)) error.value = requestFailure(reason).message;
  } finally { busy.value = false; }
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
  // A request on the previous TCP connection must not block recovery on a new one.
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
});
onUnmounted(() => {
  disposed = true; runtimeRevision++; stopRetry();
  window.removeEventListener("focus", onFocus);
  window.removeEventListener("online", onOnline);
});
</script>
<template>
  <div class="shell">
    <aside>
      <div class="brand">
        <span class="brand-icon">↗</span><span>LAN<br /><b>RemoteApp</b></span>
      </div>
      <div class="nav">▦　应用工作台</div>
      <div class="aside-bottom">
        ● 本地网络 · 加密连接
        <p>计算留在 Linux<br />窗口来到 Mac</p>
        <small>开发预览 0.1.0</small>
      </div>
    </aside>
    <main>
      <header>
        <div>
          <div class="eyebrow">YOUR APPS, A DIFFERENT DESKTOP</div>
          <h1>让工作跨越桌面</h1>
          <p>连接你的 Ubuntu，查看已安装应用和运行中的窗口。</p>
        </div>
        <span class="badge">SSH 安全传输</span>
      </header>
      <section class="connection">
        <div class="section-title">
          <h2>01 <span>连接主机</span></h2>
          <span>连接配置由本机服务保存</span>
        </div>
        <div class="fields">
          <label
            >主机 IP / DNS<input
              v-model.trim="endpoint.host"
              :disabled="busy"
              placeholder="192.168.1.20"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
              autocomplete="off" /></label
          ><label
            >SSH 用户名<input
              v-model.trim="endpoint.user"
              :disabled="busy"
              placeholder="linux-user"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
              autocomplete="off" /></label
          ><label>密码<input v-model="password" type="password" :disabled="busy" placeholder="已保存时可留空" autocomplete="current-password" /></label
          ><label class="port"
            >端口<input
              v-model.number="endpoint.port"
              :disabled="busy"
              type="number"
              min="1"
              max="65535" /></label
          ><button :disabled="!valid || busy" @click="connectHost">
            {{ busy ? "连接中…" : "连接" }}
          </button>
        </div>
        <p class="hint">
          使用这里填写的账号和密码连接，留空使用已保存配置。断网恢复后自动继续原窗口。
        </p>
      </section>
      <section class="options">
        <div>
          <h2>02 <span>原生窗口</span></h2>
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
            03 <span>已安装应用</span><span class="count">{{ apps.filter(app => app.source === "desktop").length }}</span>
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
          <h3>{{ busy ? "正在读取 Ubuntu 应用" : "Ubuntu 已安装应用将在这里" }}</h3>
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
          <h2>04 <span>运行中的窗口</span><span class="count">{{ runningConnections.length }}</span></h2>
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
        Linux 负责运行 · macOS 负责呈现
        <span>剪贴板按需传输 · 不做后台同步</span>
      </footer>
    </main>
  </div>
</template>
