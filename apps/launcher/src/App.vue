<script setup lang="ts">
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { PendingLaunches } from "./session-store";
type Endpoint = { host: string; user: string; port: number };
type AppInfo = { id: string; name: string; description: string };
type Result = {
  mode: string;
  session_id: string | null;
  display: string | null;
  pid: number;
  message: string;
};
type SessionInfo = {
  session_id: string;
  app_id: string;
  backend: string;
  display: number;
  controlled: boolean;
};
const endpoint = ref<Endpoint>({ host: "", user: "", port: 22 }),
  apps = ref<AppInfo[]>([]),
  search = ref(""),
  mode = ref("native"),
  xpraPath = ref("/Applications/Xpra.app/Contents/MacOS/Xpra");
const busy = ref(false),
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
      JSON.stringify({ endpoint: endpoint.value, xpraPath: xpraPath.value }),
    );
  } catch {}
}
try {
  const s = JSON.parse(storage?.getItem("lan-remoteapp.connection") || "null");
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
    if (typeof s.xpraPath === "string") xpraPath.value = s.xpraPath;
  }
} catch {}
const appName = (id: string) => apps.value.find((a) => a.id === id)?.name || id;
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
  sessions.value = [];
  connected.value = "";
  const sent = { ...endpoint.value };
  try {
    const [data, running] = await Promise.all([
      invoke<AppInfo[]>("list_apps", { endpoint: sent }),
      invoke<SessionInfo[]>("list_sessions", { endpoint: sent }),
    ]);
    apps.value = data;
    sessions.value = running;
    connected.value = JSON.stringify(sent);
    notice.value = data.length
      ? `已读取 ${data.length} 个获准应用，${running.length} 个运行中的会话。`
      : "主机尚未发布应用，请先配置应用白名单。";
    save();
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = false;
  }
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
async function launch(app: AppInfo) {
  if (busy.value || !catalogCurrent.value) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  const key = `${signature()}:${app.id}:${mode.value}`;
  try {
    const requestId = attempts.id(key);
    const result = await invoke<Result>("launch_app", {
      endpoint: { ...endpoint.value },
      appId: app.id,
      mode: mode.value,
      xpraPath: xpraPath.value,
      requestId,
    });
    record(app.name, result);
    attempts.resolve(key);
  } catch (e) {
    error.value =
      String(e) + " 再次点击会复用请求 ID；也可刷新并连接已有会话。";
  } finally {
    await readSessions().catch(() => {});
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
      xpraPath: xpraPath.value,
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
          <p>连接你的 Linux 主机，打开已批准的应用。</p>
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
            {{ busy ? "处理中…" : "读取应用目录 →" }}
          </button>
        </div>
        <p class="hint">
          首次连接请先在终端核验主机指纹并完成 SSH 登录。不会自动信任未知主机。
        </p>
      </section>
      <section class="options">
        <div>
          <h2>02 <span>显示方式</span></h2>
          <div class="switch">
            <button
              :class="{ selected: mode === 'xpra' }"
              :disabled="busy"
              @click="mode = 'xpra'"
            >
              Xpra 兼容模式 <small>P0</small></button
            ><button
              :class="{ selected: mode === 'native' }"
              :disabled="busy"
              @click="mode = 'native'"
            >
              原生窗口预览 <small>P1</small>
            </button>
          </div>
        </div>
        <label v-if="mode === 'xpra'" class="client-path"
          >Xpra 客户端路径<input
            v-model="xpraPath"
            :disabled="busy"
            spellcheck="false"
        /></label>
        <p v-else class="hint mode-note">
          Swift AppKit 窗口宿主，不依赖 XQuartz。尚未进行真实设备验收。
        </p>
      </section>
      <div v-if="error" class="alert error" role="alert">{{ error }}</div>
      <div v-if="notice" class="alert notice" role="status">{{ notice }}</div>
      <section class="catalog">
        <div class="section-title">
          <h2>
            03 <span>应用目录</span><span class="count">{{ apps.length }}</span>
          </h2>
          <input
            v-model="search"
            class="search"
            placeholder="搜索应用…"
            aria-label="搜索应用"
          />
        </div>
        <div v-if="!apps.length" class="empty">
          <div>▦</div>
          <h3>{{ busy ? "正在读取主机目录" : "你的 Linux 应用将在这里" }}</h3>
          <p>
            先连接主机。这里只显示服务端明确批准的应用，<br />不会从客户端执行任意命令。
          </p>
        </div>
        <div v-else class="app-grid">
          <button
            v-for="app in filtered"
            :key="app.id"
            class="app-card"
            :disabled="busy || !catalogCurrent"
            @click="launch(app)"
          >
            <span class="app-icon">{{
              app.name.slice(0, 1).toUpperCase()
            }}</span
            ><strong>{{ app.name }}</strong>
            <p>{{ app.description || app.id }}</p>
            <span class="launch-label">打开应用 <span>↗</span></span>
          </button>
        </div>
        <p v-if="apps.length && !catalogCurrent" class="hint">
          主机信息已改变，请重新读取应用目录。
        </p>
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
          ><span
            >{{ session.backend === "native" ? "原生窗口" : "Xpra" }} ·
            {{ session.controlled ? "已连接" : "可连接" }}</span
          ><code>{{ session.session_id }}</code
          ><button
            class="session-action"
            :disabled="
              busy || (session.backend === 'native' && session.controlled)
            "
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
          ><span
            >{{ entry.result.mode === "xpra" ? "Xpra" : "原生窗口" }} ·
            客户端已启动</span
          ><code v-if="entry.result.session_id"
            >会话 {{ entry.result.session_id }}</code
          >
        </div>
        <p class="hint">请以实际窗口确认连接成功。</p>
      </section>
      <footer>
        Linux 负责运行 · macOS 负责呈现
        <span>文件、音频及剪贴板转发默认关闭</span>
      </footer>
    </main>
  </div>
</template>
