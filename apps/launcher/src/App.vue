<script setup lang="ts">
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { PendingLaunches } from "./session-store";
import { launchFailure } from "./launch-failure";
import AppIcon from "./AppIcon.vue";
type Endpoint = { host: string; user: string; port: number };
type LaunchTarget = { id: string; name: string; unavailable_reason?: string | null };
type AppInfo = LaunchTarget & { description: string; icon_png?: string | null; source: "desktop" | "custom" };
type AppCatalog = { apps: AppInfo[]; discovery_error: string | null };
type DesktopWindow = LaunchTarget & { window_id: number; title: string; icon_png?: string | null; minimized: boolean; controlled: boolean; session_id?: string | null };
type DesktopWindows = { windows: DesktopWindow[]; unavailable_reason: string | null };
type Result = {
  session_id: string | null;
  display: string | null;
  pid: number;
  message: string;
};
type SessionInfo = {
  session_id: string;
  app_id: string;
  backend: "native";
  display: number;
  controlled: boolean;
};
const endpoint = ref<Endpoint>({ host: "", user: "", port: 22 }),
  apps = ref<AppInfo[]>([]),
  desktopWindows = ref<DesktopWindow[]>([]),
  discoveryWarning = ref(""),
  windowWarning = ref(""),
  search = ref("");
const busy = ref(false),
  renderScale = ref("auto"),
  error = ref(""),
  notice = ref(""),
  connected = ref(""),
  sessions = ref<SessionInfo[]>([]),
  launches = ref<{ name: string; result: Result }[]>([]);
let storage: Storage | undefined;
try {
  storage = localStorage;
} catch {}
const attempts = new PendingLaunches(storage);
const filtered = computed(() =>
  apps.value.filter((a) =>
    `${a.name} ${a.id}`.toLowerCase().includes(search.value.toLowerCase()),
  ),
);
const installed = computed(() => filtered.value.filter(app => app.source === "desktop"));
const custom = computed(() => filtered.value.filter(app => app.source === "custom"));
const filteredWindows = computed(() => desktopWindows.value.filter(window =>
  `${window.name} ${window.title}`.toLowerCase().includes(search.value.toLowerCase())));
const signature = () => JSON.stringify(endpoint.value),
  catalogCurrent = computed(() => connected.value === signature());
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
    localStorage.setItem(
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
const appName = (id: string) => apps.value.find((a) => a.id === id)?.name
  || desktopWindows.value.find(window => window.id === id)?.name || id;
async function readSessions() {
  const sent = { ...endpoint.value };
  const data = await invoke<SessionInfo[]>("list_sessions", { endpoint: sent });
  if (signature() === JSON.stringify(sent)) sessions.value = data;
}
async function load() {
  if (!valid.value || busy.value) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  apps.value = [];
  desktopWindows.value = [];
  discoveryWarning.value = "";
  windowWarning.value = "";
  sessions.value = [];
  connected.value = "";
  const sent = { ...endpoint.value };
  try {
    const [catalog, running] = await Promise.all([
      invoke<AppCatalog>("list_apps", { endpoint: sent }),
      invoke<SessionInfo[]>("list_sessions", { endpoint: sent }),
    ]);
    if (signature() !== JSON.stringify(sent)) return;
    apps.value = catalog.apps;
    discoveryWarning.value = catalog.discovery_error || "";
    sessions.value = running;
    connected.value = JSON.stringify(sent);
    await readWindows();
    notice.value = `已读取 ${catalog.apps.filter(app => app.source === "desktop").length} 个已安装应用，${desktopWindows.value.length} 个桌面窗口，${running.length} 个远程会话。`;
    save();
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = false;
  }
}
async function readWindows() {
  const sent = { ...endpoint.value };
  try {
    const result = await invoke<DesktopWindows>("list_desktop_windows", { endpoint: sent });
    if (signature() !== JSON.stringify(sent)) return;
    desktopWindows.value = result.windows;
    windowWarning.value = result.unavailable_reason || "";
  } catch (error) {
    desktopWindows.value = [];
    windowWarning.value = `运行窗口读取失败：${String(error)}`;
  }
}
async function refreshWindows() {
  if (busy.value || !catalogCurrent.value) return;
  busy.value = true;
  try { await readWindows(); } finally { busy.value = false; }
}
async function refresh() {
  if (busy.value || !catalogCurrent.value) return;
  busy.value = true;
  error.value = "";
  try {
    await readSessions();
  } catch (e) {
    error.value = String(e);
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
  if (busy.value || !catalogCurrent.value || app.unavailable_reason) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  const key = `${signature()}:${app.id}:native:${renderScale.value}`;
  try {
    const requestId = attempts.id(key);
    const result = await invoke<Result>("launch_app", {
      endpoint: { ...endpoint.value },
      appId: app.id,
      requestId,
      renderScale: renderScale.value === "auto" ? null : Number(renderScale.value),
    });
    record(app.name, result);
    attempts.resolve(key);
  } catch (e) {
    const failure = launchFailure(e);
    if (!failure.retrySameRequest) attempts.resolve(key);
    error.value = failure.message + (failure.retrySameRequest
      ? " 可先刷新会话；再次点击会安全重试同一次启动。" : "");
  } finally {
    await Promise.all([readSessions().catch(() => {}), readWindows()]);
    busy.value = false;
  }
}
async function resume(session: SessionInfo) {
  if (busy.value || !catalogCurrent.value) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  try {
    const result = await invoke<Result>("resume_session", {
      endpoint: { ...endpoint.value },
      sessionId: session.session_id,
    });
    record(appName(session.app_id), result);
  } catch (e) {
    error.value = String(e);
  } finally {
    await readSessions().catch(() => {});
    busy.value = false;
  }
}
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
          <span>不保存密码或私钥</span>
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
          ><label class="port"
            >端口<input
              v-model.number="endpoint.port"
              :disabled="busy"
              type="number"
              min="1"
              max="65535" /></label
          ><button :disabled="!valid || busy" @click="load">
            {{ busy ? "处理中…" : "读取应用与窗口 →" }}
          </button>
        </div>
        <p class="hint">
          首次连接请先在终端核验主机指纹并完成 SSH 登录。不会自动信任未知主机。
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
          <input
            v-model="search"
            class="search"
            placeholder="搜索应用或窗口…"
            aria-label="搜索应用或窗口"
          />
        </div>
        <p v-if="discoveryWarning" class="alert error" role="status">{{ discoveryWarning }}</p>
        <div v-if="!apps.some(app => app.source === 'desktop')" class="empty">
          <div>▦</div>
          <h3>{{ busy ? "正在读取 Ubuntu 应用" : "Ubuntu 已安装应用将在这里" }}</h3>
          <p>
            连接后读取 Ubuntu 的应用名称和图标。安装或卸载应用后，重新读取即可更新。
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
            <p>{{ app.unavailable_reason || app.description || app.id }}</p>
            <span class="launch-label">{{ app.unavailable_reason ? "暂不可启动" : "打开应用 ↗" }}</span>
          </button>
        </div>
        <p v-if="apps.some(app => app.source === 'desktop') && !installed.length" class="hint">没有匹配的应用。</p>
        <p v-if="apps.length && !catalogCurrent" class="hint">
          主机信息已改变，请重新读取应用与窗口。
        </p>
      </section>
      <section v-if="catalogCurrent" class="catalog desktop-windows">
        <div class="section-title">
          <h2>04 <span>运行中的桌面窗口</span><span class="count">{{ desktopWindows.length }}</span></h2>
          <button class="session-action" :disabled="busy" @click="refreshWindows">刷新窗口</button>
        </div>
        <p class="hint">接管现有窗口会保留登录状态和内容，首次控制可能需要在 Ubuntu 确认授权。支持 X11 / XWayland；原生 Wayland 窗口暂不能接管。</p>
        <p v-if="windowWarning" class="alert error" role="status">{{ windowWarning }}</p>
        <p v-else-if="!desktopWindows.length" class="hint">当前桌面没有可接管的窗口。</p>
        <p v-else-if="!filteredWindows.length" class="hint">没有匹配的窗口。</p>
        <div v-for="window in filteredWindows" :key="window.id" class="desktop-window-row">
          <AppIcon :icon="window.icon_png" :name="window.name" />
          <div class="window-label"><strong>{{ window.name }}</strong><p :title="window.title">{{ window.title || "无标题窗口" }}</p></div>
          <span v-if="window.minimized" class="window-state">已最小化</span>
          <button class="session-action" :disabled="busy || window.controlled || Boolean(window.session_id)" @click="launch(window)">{{ window.controlled ? "已连接" : window.session_id ? "已有远程会话" : "接管窗口" }}</button>
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
      <section v-if="catalogCurrent" class="history sessions">
        <div class="section-title">
          <h2>
            运行中的会话 <span class="count">{{ sessions.length }}</span>
          </h2>
          <button class="session-action" :disabled="busy" @click="refresh">
            刷新会话
          </button>
        </div>
        <p v-if="!sessions.length" class="hint">
          当前没有运行中的会话。打开应用后可在这里重新连接。
        </p>
        <div
          v-for="session in sessions"
          :key="session.session_id"
          class="session-row"
        >
          <b>{{ appName(session.app_id) }}</b
          ><span>原生窗口 · {{ session.controlled ? "已连接" : "可连接" }}</span
          ><code>{{ session.session_id }}</code
          ><button
            class="session-action"
            :disabled="busy || session.controlled"
            @click="resume(session)"
          >
            重新连接
          </button>
        </div>
        <p class="hint">
          断开宿主会保留远端应用。重启启动器后，读取同一主机即可找回会话。
        </p>
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
        <span>文件、音频和后台剪贴板同步关闭</span>
      </footer>
    </main>
  </div>
</template>
