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
import type { AppCatalog, AppInfo, DesktopMonitor, DesktopMonitors, DesktopWindow, DesktopWindows, Endpoint, LaunchTarget, SessionInfo } from "./launcher-types";
import AppIcon from "./AppIcon.vue";
import { retireLegacyPreferences } from "./connection-preferences";
import { SavedConnections } from "./saved-connections";
import { AppShortcutStore } from "./app-shortcuts";
import type { SavedConnection } from "./saved-connections";
import { automaticApplications, emptyAutomaticStatus } from "./automatic-windows";
import type { AutomaticMode, AutomaticRule, AutomaticStatus } from "./automatic-windows";
type Result = {
  session_id: string | null;
  display: string | null;
  pid: number;
  message: string;
};
const navigation = [
  { id: "apps", label: "应用工作台", eyebrow: "APPLICATIONS", description: "打开其他设备的应用，继续正在进行的工作。", icon: "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z" },
  { id: "devices", label: "设备管理", eyebrow: "DEVICES", description: "配对新设备，管理每台设备的连接。", icon: "M3 4h18v12H3zM8 21h8M12 16v5" },
  { id: "sharing", label: "本机共享", eyebrow: "SHARING", description: "让已配对设备使用这台电脑上的应用。", icon: "M12 16V3m-5 5 5-5 5 5M4 13v7h16v-7" },
  { id: "display", label: "显示设置", eyebrow: "PREFERENCES", description: "调整新打开的远程应用窗口。", icon: "M4 7h9m4 0h3M4 17h3m4 0h9M13 4v6M7 14v6" },
  { id: "history", label: "连接记录", eyebrow: "ACTIVITY", description: "查看本次打开启动器后的应用连接记录。", icon: "M12 8v5l3 2M3 12a9 9 0 1 0 3-6.7M3 3v5h5" },
] as const;
type Page = typeof navigation[number]["id"];
const activePage = ref<Page>("apps");
const currentPage = computed(() => navigation.find(item => item.id === activePage.value) ?? navigation[0]);
const endpoint = ref<Endpoint>({ peerId: "" }),
  apps = ref<AppInfo[]>([]),
  desktopWindows = ref<DesktopWindow[]>([]),
  desktopMonitors = ref<DesktopMonitor[]>([]),
  discoveryWarning = ref(""),
  windowWarning = ref(""),
  sessionWarning = ref(""),
  monitorWarning = ref(""),
  search = ref("");
type PictureMode = "adapt" | "source";
const pictureMode = ref<PictureMode>("adapt");
const pictureModeReady = ref(false);
const pictureModeSaving = ref(false);
const pictureModeError = ref("");
async function readPictureMode() {
  try { pictureMode.value = await invoke<PictureMode>("picture_mode"); pictureModeReady.value = true; }
  catch { pictureModeError.value = "画面尺寸设置读取失败，请重新打开启动器。"; }
}
async function savePictureMode(event: Event) {
  const mode = (event.target as HTMLSelectElement).value as PictureMode;
  pictureModeSaving.value = true; pictureModeError.value = "";
  try { await invoke("set_picture_mode", {mode}); pictureMode.value = mode; }
  catch { pictureModeError.value = "画面尺寸设置保存失败，请重试。"; }
  finally { (event.target as HTMLSelectElement).value = pictureMode.value; pictureModeSaving.value = false; }
}
type VideoColor = "420" | "444-atlas-v1";
const videoColor = ref<VideoColor>("420"), videoColorReady = ref(false), videoColorSaving = ref(false), videoColorError = ref("");
async function readVideoColor() {
  try { videoColor.value = await invoke<VideoColor>("video_color"); videoColorReady.value = true; }
  catch { videoColorError.value = "色彩设置读取失败，请重新打开启动器。"; }
}
async function saveVideoColor(event: Event) {
  const mode = (event.target as HTMLSelectElement).value as VideoColor;
  videoColorSaving.value = true; videoColorError.value = "";
  try { await invoke("set_video_color", {mode}); videoColor.value = mode; }
  catch { videoColorError.value = "色彩设置保存失败，请重试。"; }
  finally { (event.target as HTMLSelectElement).value = videoColor.value; videoColorSaving.value = false; }
}
const busy = ref(false),
  videoFPS = ref<30 | 60>(60),
  applicationScene = ref(false),
  error = ref(""),
  notice = ref(""),
  catalogSavedAt = ref<number | null>(null),
  catalogPersisted = ref(false),
  connected = ref(""),
  runtimeEndpoint = ref(""),
  attemptedEndpoint = ref(""),
  actionConnectionError = ref(false),
  sessions = ref<SessionInfo[]>([]),
  launches = ref<{ name: string; result: Result; deviceName: string; deviceAddress: string; peerId: string; createdAt: number }[]>([]);
let storage: Storage | undefined;
try {
  storage = localStorage;
} catch {}
retireLegacyPreferences(storage);
const attempts = new PendingLaunches(storage);
const savedConnectionStore = new SavedConnections(storage);
const savedConnections = ref(savedConnectionStore.list());
const appShortcutStore = new AppShortcutStore(storage);
const appShortcuts = ref(appShortcutStore.list());
const shortcutWarning = ref("");
const catalogView = ref<"all" | "favorites" | "recent">("all");
const catalogExpanded = ref(false);
const connectionNames = ref<Record<string, string>>({});
const savedConnectionError = ref(""), savedConnectionNotice = ref("");
const switchingConnection = ref("");
type StartingApp = { endpoint: string; appId: string; name: string; requestKey: string; missingReads: number };
const startingApps = ref<StartingApp[]>([]), launchingAppId = ref<string | null>(null);
type LocalNode = {device_id:string; name:string; platform?:string; sharing_enabled:boolean; permissions?:{screen_recording:boolean; accessibility:boolean}};
const localNode = ref<LocalNode|null>(null), localWarning=ref(""), localBusy=ref(false);
type NativePeer = {device_id:string; name:string; address:string; fingerprint:string; connected:boolean};
const nativePeers = ref<NativePeer[]>([]), nativeWarning=ref(""), pairAddress=ref(""), pairCode=ref(""), ownInvite=ref(""), pairing=ref(false);
const pairNotice = ref("");
const selectedPeer = computed(() => nativePeers.value.find(peer => peer.device_id === endpoint.value.peerId));
function peerLabel(peer: NativePeer): string {
  return savedConnections.value.find(row => row.peerId === peer.device_id)?.name || peer.name;
}
const networkReady = ref(false);
let networkRevision = 0;
const automaticStatus = ref<AutomaticStatus>(emptyAutomaticStatus());
const automaticApp = ref("");
const automaticSaving = ref(false);
const automaticError = ref("");
const takeoverError = ref("");
let automaticRevision = 0;
const automaticOptions = computed(() => automaticApplications(apps.value, desktopWindows.value, automaticStatus.value.rules, automaticStatus.value.connectedApplications));
const automaticKey = computed(() => automaticOptions.value.some(app => app.id === automaticApp.value)
  ? automaticApp.value : automaticOptions.value.find(app => automaticStatus.value.connectedApplications.includes(app.id))?.id || automaticOptions.value[0]?.id || "");
const automaticUnavailable = computed(() => automaticOptions.value.find(app => app.id === automaticKey.value)?.unavailableReason || "");
const automaticRule = computed<AutomaticRule>(() => automaticStatus.value.rules.find(
  item => item.applicationKey === automaticKey.value)?.rule || { mode: "off", applications: [] });
async function readAutomatic() {
  if (!valid.value || automaticSaving.value) return;
  const sent = { ...endpoint.value }, revision = ++automaticRevision;
  try {
    const result = await invoke<AutomaticStatus>("automatic_window_status", { endpoint: sent });
    if (!disposed && signature() === endpointKey(sent) && revision === automaticRevision) {
      automaticStatus.value = result; automaticError.value = "";
    }
  } catch {
    if (!disposed && signature() === endpointKey(sent) && revision === automaticRevision)
      automaticError.value = "自动接入设置暂时无法读取。";
  }
}
async function saveAutomatic(rule: AutomaticRule) {
  if (automaticSaving.value || automaticStatus.value.takeoverEnabled || !valid.value || !automaticKey.value) return;
  const selected = automaticOptions.value.find(app => app.id === automaticKey.value)!;
  if (rule.mode !== "off" && selected.unavailableReason) return;
  const sent = { ...endpoint.value }, revision = ++automaticRevision;
  automaticSaving.value = true; automaticError.value = "";
  try {
    const result = await invoke<AutomaticStatus>("set_automatic_window_rule", {
      preference: { peerId: sent.peerId, applicationKey: selected.id, name: selected.name, rule },
    });
    if (!disposed && signature() === endpointKey(sent) && revision === automaticRevision)
      automaticStatus.value = result;
  } catch {
    if (!disposed && signature() === endpointKey(sent) && revision === automaticRevision)
      automaticError.value = "自动接入设置保存失败，原设置保留，请重试。";
  } finally { automaticSaving.value = false; }
}
async function deviceWindowAction(disconnect = false) {
  if (automaticSaving.value || !valid.value) return;
  const sent = { ...endpoint.value }, revision = ++automaticRevision;
  automaticSaving.value = true; takeoverError.value = "";
  try {
    const result = await invoke<AutomaticStatus>(disconnect ? "disconnect_device_windows" : "set_device_window_takeover", {
      endpoint: sent, ...(!disconnect && { enabled: !automaticStatus.value.takeoverEnabled, videoFps: videoFPS.value }),
    });
    if (!disposed && signature() === endpointKey(sent) && revision === automaticRevision)
      automaticStatus.value = result;
  } catch {
    if (!disposed && signature() === endpointKey(sent) && revision === automaticRevision)
      takeoverError.value = disconnect ? "断开窗口失败，请重试。" : "全窗口接管设置保存失败，原设置保留，请重试。";
  } finally { automaticSaving.value = false; }
}
function changeAutomaticMode(event: Event) {
  const mode = (event.target as HTMLSelectElement).value as AutomaticMode;
  void saveAutomatic({ mode, applications: [...automaticRule.value.applications] });
}
function changeAutomaticApplication(id: string, event: Event) {
  const applications = new Set(automaticRule.value.applications);
  if ((event.target as HTMLInputElement).checked) applications.add(id); else applications.delete(id);
  void saveAutomatic({ mode: "selected", applications: [...applications] });
}
const addressEdits=ref<Record<string,string>>({});
type AudioStatus = { enabled: boolean; state: string; message: string };
const deviceAudio = ref<Record<string, AudioStatus>>({});
const audioSaving = ref<Record<string, boolean>>({});
const audioErrors = ref<Record<string, string>>({});
let audioRevision = 0;
async function readAudio() {
  const revision = audioRevision;
  try {
    const status = await invoke<Record<string, AudioStatus>>("device_audio_status");
    if (!disposed && revision === audioRevision) deviceAudio.value = status;
  } catch { /* Keep the last known setting; an update reports a visible error. */ }
}
function audioLabel(id: string) {
  const status = deviceAudio.value[id];
  if (!status?.enabled) return "已关闭";
  return ({ connecting: "正在连接声音…", playing: "已接收系统声音", unavailable: "声音暂不可用，自动重试中", unsupported: "发布端需要更新" } as Record<string, string>)[status.state] || "正在连接声音…";
}
async function setAudio(id: string, event: Event) {
  const input = event.target as HTMLInputElement;
  const enabled = input.checked;
  if (audioSaving.value[id]) return;
  audioSaving.value[id] = true; audioErrors.value[id] = ""; audioRevision++;
  try { deviceAudio.value[id] = await invoke<AudioStatus>("set_device_audio", { endpoint: { peerId: id }, enabled }); }
  catch (error) { audioErrors.value[id] = typeof error === "string" ? error : "声音设置保存失败，请重试。"; }
  finally { input.checked = deviceAudio.value[id]?.enabled ?? false; audioSaving.value[id] = false; audioRevision++; }
}
let inviteTimer:ReturnType<typeof setTimeout>|undefined;
async function networkAction(body:Record<string,unknown>) { return invoke<any>("network_action",{body}); }
async function readNetwork() {
  void readAudio();
  if (switchingConnection.value) return;
  const revision = ++networkRevision;
  try { const status=await networkAction({op:"status"});
    if (disposed || revision !== networkRevision) return;
    nativePeers.value=status.peers; networkReady.value = true;
    for(const peer of nativePeers.value) {
      const saved = savedConnections.value.find(row => row.peerId === peer.device_id);
      addressEdits.value[peer.device_id] ??= saved?.address || peer.address;
      connectionNames.value[peer.device_id] ??= saved?.name || peer.name;
    }
    nativeWarning.value=""; }
  catch { if (!disposed && revision === networkRevision) nativeWarning.value="直接连接组件尚未就绪。"; }
}
async function inviteDevice() {
  pairing.value=true;
  try { ownInvite.value=(await networkAction({op:"invite"})).invitation;
    if(inviteTimer) clearTimeout(inviteTimer); inviteTimer=setTimeout(()=>ownInvite.value="",300000);
  } catch { nativeWarning.value="邀请生成失败，请重试。"; } finally {pairing.value=false;}
}
async function pairDevice() {
  pairing.value=true;
  pairNotice.value="";
  try { await networkAction({op:"pair",address:pairAddress.value.trim(),invitation:pairCode.value.trim()}); pairCode.value=""; await readNetwork(); pairNotice.value="设备已双向配对，点击“查看应用”即可开始使用。"; }
  catch {nativeWarning.value="配对未完成，请检查设备 IP、端口和五分钟内生成的邀请。身份变更需先撤销旧配对。";}
  finally {pairing.value=false;}
}
async function selectPeer(peer:NativePeer) {
  if (busy.value || pairing.value || switchingConnection.value) return;
  activePage.value="apps";
  endpoint.value={peerId:peer.device_id};
  await nextTick(); await load();
}
function savePeerConnection(peer: NativePeer) {
  savedConnectionError.value = ""; savedConnectionNotice.value = "";
  try {
    savedConnectionStore.save({ peerId: peer.device_id,
      name: connectionNames.value[peer.device_id] ?? peer.name,
      address: addressEdits.value[peer.device_id] ?? peer.address });
    savedConnections.value = savedConnectionStore.list();
    savedConnectionNotice.value = "连接已保存，重启后仍可在“已保存连接”中一键切换。";
  } catch (error) { savedConnectionError.value = (error as Error).message; }
}
function removeSavedConnection(row: SavedConnection) {
  savedConnectionError.value = ""; savedConnectionNotice.value = "";
  try {
    savedConnectionStore.remove(row.peerId);
    savedConnections.value = savedConnectionStore.list();
    savedConnectionNotice.value = "已移除保存项，设备配对和已打开窗口仍保留。";
  } catch (error) { savedConnectionError.value = (error as Error).message; }
}
async function connectSaved(row: SavedConnection) {
  if (busy.value || pairing.value || switchingConnection.value) return;
  savedConnectionError.value = ""; savedConnectionNotice.value = "";
  const peer = nativePeers.value.find(peer => peer.device_id === row.peerId);
  if (!peer) {
    savedConnectionError.value = "这台设备尚未配对或配对已撤销，请在设备管理中重新配对。";
    return;
  }
  switchingConnection.value = row.peerId; busy.value = true; pairing.value = true;
  ++networkRevision;
  try {
    if (peer.address !== row.address) {
      await networkAction({ op: "address", device_id: row.peerId, address: row.address });
      peer.address = row.address;
      addressEdits.value[row.peerId] = row.address;
    }
    const sameDevice = endpoint.value.peerId === row.peerId;
    endpoint.value = { peerId: row.peerId };
    activePage.value = "apps";
    await nextTick();
    if (sameDevice) restoreCatalog();
    busy.value = false;
    await load();
  } catch { savedConnectionError.value = "连接切换失败，请检查保存的地址和设备共享状态后重试。"; }
  finally { busy.value = false; pairing.value = false; switchingConnection.value = ""; }
}
async function changePeer(event: Event) {
  const peer = nativePeers.value.find(item => item.device_id === (event.target as HTMLSelectElement).value);
  if (!peer) return;
  const saved = savedConnections.value.find(row => row.peerId === peer.device_id);
  if (saved) await connectSaved(saved); else await selectPeer(peer);
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
let localRevision = 0;
async function readLocal() {
  // Polls can finish out of order, including after a sharing/permission change.
  // Never let an old observation overwrite a confirmed user action.
  if (localBusy.value) return;
  const revision = ++localRevision;
  try {
    const info = await invoke<LocalNode>("local_node_info");
    if (disposed || revision !== localRevision) return;
    localNode.value = info; localWarning.value = "";
  } catch {
    if (!disposed && revision === localRevision) localWarning.value = "本机共享组件尚未就绪。";
  }
}
async function toggleSharing() {
  if (localBusy.value || !localNode.value) return;
  ++localRevision;
  localBusy.value=true;
  try {localNode.value=await invoke<LocalNode>("set_local_sharing",{enabled:!localNode.value?.sharing_enabled});
  } catch {localWarning.value="共享状态未能更新，请重试。";} finally {localBusy.value=false;}
}
async function requestPermissions() {
  if (localBusy.value) return;
  ++localRevision;
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
const selectedShortcuts = computed(() => appShortcuts.value.find(row => row.peerId === endpoint.value.peerId));
const favoriteIds = computed(() => new Set(selectedShortcuts.value?.favorites || []));
const favoriteApps = computed(() => (selectedShortcuts.value?.favorites || []).flatMap(id =>
  apps.value.filter(app => app.id === id && app.source === "desktop")));
const recentApps = computed(() => (selectedShortcuts.value?.recent || []).flatMap(id =>
  apps.value.filter(app => app.id === id && app.source === "desktop")));
const quickApps = computed(() => [...favoriteApps.value, ...recentApps.value.filter(app => !favoriteIds.value.has(app.id))]
  .filter(app => `${app.name} ${app.id}`.toLowerCase().includes(search.value.toLowerCase())).slice(0, 6));
const catalogApps = computed(() => catalogView.value === "all" ? installed.value :
  (catalogView.value === "favorites" ? favoriteApps.value : recentApps.value).filter(app =>
    `${app.name} ${app.id}`.toLowerCase().includes(search.value.toLowerCase())));
const visibleApps = computed(() => catalogExpanded.value || search.value.trim() ? catalogApps.value : catalogApps.value.slice(0, 12));
function toggleFavorite(app: AppInfo) {
  try {
    appShortcutStore.toggleFavorite(endpoint.value.peerId, app.id);
    appShortcuts.value = appShortcutStore.list(); shortcutWarning.value = "";
  } catch (error) { shortcutWarning.value = (error as Error).message; }
}
const runningConnections = computed(() => connections(apps.value, desktopWindows.value, sessions.value));
const currentStarting = computed(() => startingApps.value.filter(item => item.endpoint === signature()));
const filteredStarting = computed(() => currentStarting.value.filter(item =>
  item.name.toLowerCase().includes(search.value.toLowerCase())));
const isStarting = (appId: string) => currentStarting.value.some(item => item.appId === appId);
const desktopSession = (monitor: DesktopMonitor) => sessions.value.find(
  session => session.mode === "desktop" && session.monitor_id === monitor.id);
const filteredConnections = computed(() => runningConnections.value.filter(row =>
  `${row.name} ${row.title}`.toLowerCase().includes(search.value.toLowerCase()))
  .sort((a, b) => Number(b.controlled) - Number(a.controlled)));
const signature = () => endpointKey(endpoint.value),
  catalogCurrent = computed(() => connected.value === signature()),
  runtimeCurrent = computed(() => runtimeEndpoint.value === signature());
let runtimeRevision = 0;
let focusRead: Promise<void> | undefined;
let recoveryTimer: ReturnType<typeof setTimeout> | undefined;
let startupTimer: ReturnType<typeof setTimeout> | undefined;
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
function stopStartupPoll() {
  if (startupTimer !== undefined) clearTimeout(startupTimer);
  startupTimer = undefined;
}
function scheduleStartupPoll() {
  if (disposed || startupTimer !== undefined || !valid.value || !currentStarting.value.length) return;
  startupTimer = setTimeout(() => {
    startupTimer = undefined;
    if (busy.value || focusRead) scheduleStartupPoll();
    else void readRuntime();
  }, 2000);
}
function reconcileStarting(sent: Endpoint, remote: SessionInfo[]) {
  const target = endpointKey(sent);
  const ready: StartingApp[] = [], ended: StartingApp[] = [], failed: string[] = [];
  startingApps.value = startingApps.value.flatMap(item => {
    if (item.endpoint !== target) return [item];
    const matching = remote.filter(session => session.app_id === item.appId && session.mode !== "desktop");
    if (matching.some(session => (session.window_count ?? 0) > 0)) {
      ready.push(item);
      attempts.resolve(item.requestKey);
      return [];
    }
    const unavailable = matching.find(session => session.unavailable_reason)?.unavailable_reason;
    if (unavailable) {
      const message = unavailable === "window_size_unavailable" ? "窗口尺寸约束超出采集范围"
        : unavailable === "window_resize_failed" ? "窗口未接受尺寸调整" : "窗口暂不可用";
      failed.push(`${item.name}：${message}，原会话已保留，可调整后重试。`);
      attempts.resolve(item.requestKey);
      return [];
    }
    if (!matching.length && item.missingReads >= 1) {
      ended.push(item);
      attempts.resolve(item.requestKey);
      return [];
    }
    return [{ ...item, missingReads: matching.length ? 0 : item.missingReads + 1 }];
  });
  if (ready.length) notice.value = `${ready.map(item => item.name).join("、")}已出现窗口，可在“运行中的窗口”点击连接。`;
  if (ended.length) {
    if (!ready.length) notice.value = "";
    error.value = `${ended.map(item => item.name).join("、")}启动后未产生窗口，会话已结束；可以再次打开。`;
  }
  if (failed.length) { notice.value = ""; error.value = failed.join("；"); }
  if (currentStarting.value.length) scheduleStartupPoll();
  else stopStartupPoll();
}
const valid = computed(() => validPeerId(endpoint.value.peerId));
function save() {
  try {
    storage?.setItem(
      "lan-remoteapp.connection",
      JSON.stringify({ endpoint: endpoint.value, applicationScene: applicationScene.value, videoFPS: videoFPS.value }),
    );
  } catch {}
}
try {
  const saved = JSON.parse(storage?.getItem("lan-remoteapp.connection") || "null");
  applicationScene.value = saved?.applicationScene === true;
  videoFPS.value = saved?.videoFPS === 30 ? 30 : 60;
  if (validPeerId(saved?.endpoint?.peerId)) endpoint.value = { peerId: saved.endpoint.peerId };
} catch {}
function restoreCatalog() {
  ++automaticRevision;
  automaticStatus.value = emptyAutomaticStatus(); automaticApp.value = ""; automaticError.value = ""; takeoverError.value = "";
  stopRetry();
  stopStartupPoll();
  const cached = catalogCache.get(signature());
  apps.value = cached?.apps ?? [];
  connected.value = cached ? signature() : "";
  catalogSavedAt.value = cached?.savedAt ?? null;
  catalogPersisted.value = catalogCache.isPersisted(signature());
  desktopWindows.value = [];
  desktopMonitors.value = [];
  sessions.value = [];
  runtimeEndpoint.value = "";
  attemptedEndpoint.value = cached ? signature() : "";
  actionConnectionError.value = false;
  runtimeRevision++;
  focusRead = undefined;
  discoveryWarning.value = "";
  windowWarning.value = "";
  sessionWarning.value = "";
  monitorWarning.value = "";
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
      // The explicit Refresh Apps action reads only the application directory.
      ...(!refreshApps ? [readRuntime(sent)] : []),
    ]);
    if (signature() !== endpointKey(sent)) return;
    const failures: string[] = [];
    if (catalog.status === "fulfilled") {
      apps.value = catalog.value.catalog.apps;
      discoveryWarning.value = catalog.value.catalog.discovery_error || "";
      catalogSavedAt.value = catalog.value.savedAt;
      catalogPersisted.value = catalog.value.persisted;
      connected.value = endpointKey(sent);
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
  const [running, desktop, monitors] = await Promise.allSettled([
    invoke<SessionInfo[]>("list_sessions", { endpoint: sent }),
    invoke<DesktopWindows>("list_desktop_windows", { endpoint: sent }),
    invoke<DesktopMonitors>("list_desktop_monitors", { endpoint: sent }),
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
  if (monitors.status === "fulfilled") {
    desktopMonitors.value = monitors.value.monitors;
    monitorWarning.value = monitors.value.unavailable_reason || "";
  } else {
    desktopMonitors.value = [];
    monitorWarning.value = "这台设备暂不支持整桌面模式。";
  }
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
  if (running.status === "fulfilled") reconcileStarting(sent, running.value);
  else scheduleStartupPoll();
  void readAutomatic();
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
function record(name: string, result: Result, appId?: string, applicationKey?: string | null) {
  launches.value.unshift({ name, result, deviceName: selectedPeer.value?.name || endpoint.value.peerId, deviceAddress: selectedPeer.value?.address || "", peerId: endpoint.value.peerId, createdAt: Date.now() });
  launches.value = launches.value.slice(0, 20);
  const app = apps.value.find(item => item.source === "desktop" && (item.id === appId || item.id === applicationKey));
  if (app) {
    try {
      appShortcutStore.recordUse(endpoint.value.peerId, app.id);
      appShortcuts.value = appShortcutStore.list(); shortcutWarning.value = "";
    } catch (error) { shortcutWarning.value = (error as Error).message; }
  }
  notice.value = result.message;
  save();
}
function useApplicationScene(appId: string): boolean {
  if (!applicationScene.value || pictureMode.value === "source") return false;
  const existing = sessions.value.filter(session => session.app_id === appId && session.mode !== "desktop");
  // Private application desktops retain their existing presentation. Only a
  // publisher-advertised live source can opt into the composed scene.
  if (existing.some(session => session.display !== 0)) return false;
  const exact = desktopWindows.value.find(window => window.id === appId);
  const candidates = exact ? [exact] : desktopWindows.value.filter(window => window.app_id === appId ||
    existing.some(session => window.session_id === session.session_id));
  return candidates.length === 1 && candidates[0].application_scene === true;
}
async function launch(app: LaunchTarget) {
  if (busy.value || !valid.value || app.unavailable_reason || isStarting(app.id)) return;
  busy.value = true;
  launchingAppId.value = app.id;
  error.value = "";
  actionConnectionError.value = false;
  notice.value = "";
  const sent = { ...endpoint.value };
  const scale = 1;
  const useScene = useApplicationScene(app.id);
  // Fixed canvas follows the local work area at launch. Local resizing scales
  // the composed image; it doesn't change popup placement behind the user's back.
  const canvasScale = Math.min(1, 4096 / window.screen.availWidth, 4096 / Math.max(240, window.screen.availHeight - 40),
    Math.sqrt(8_000_000 / (window.screen.availWidth * Math.max(240, window.screen.availHeight - 40))));
  const scene = useScene ? {
    width: Math.max(640, Math.floor(window.screen.availWidth * canvasScale)),
    height: Math.max(480, Math.floor(Math.max(240, window.screen.availHeight - 40) * canvasScale)),
  } : null;
  const key = `${signature()}:${app.id}:native:${scale}${scene ? `:scene:${scene.width}x${scene.height}` : ""}`;
  try {
    const requestId = attempts.id(key);
    const result = await invoke<Result>("launch_app", {
      endpoint: sent,
      appId: app.id,
      appName: app.name,
      requestId,
      renderScale: scale,
      videoFps: videoFPS.value,
      scene,
      iconPng: app.icon_png ?? null,
      applicationKey: app.application_key || app.id,
    });
    attempts.resolve(key);
    startingApps.value = startingApps.value.filter(item => item.endpoint !== endpointKey(sent) || item.appId !== app.id);
    if (signature() === endpointKey(sent)) record(app.name, applicationScene.value && !useScene
      ? { ...result, message: `${result.message} 当前入口使用兼容窗口显示。` } : result, app.id, app.application_key);
  } catch (e) {
    const failure = launchFailure(e);
    if (!failure.retrySameRequest) attempts.resolve(key);
    if (signature() === endpointKey(sent)) {
      actionConnectionError.value = isConnectionFailure(e);
      if (failure.code === "application_window_pending") {
        startingApps.value = [...startingApps.value.filter(item => item.endpoint !== endpointKey(sent) || item.appId !== app.id),
          { endpoint: endpointKey(sent), appId: app.id, name: app.name, requestKey: key, missingReads: 0 }];
        notice.value = `${app.name}启动中，正在等待应用窗口；窗口出现后会自动显示连接入口。`;
      } else {
        if (!failure.retrySameRequest) startingApps.value = startingApps.value.filter(item => item.endpoint !== endpointKey(sent) || item.appId !== app.id);
        error.value = failure.message + (failure.retrySameRequest
          ? " 可先刷新运行状态；再次点击会安全重试同一次启动。" : "");
      }
    }
  } finally {
    await readRuntime(sent);
    launchingAppId.value = null;
    busy.value = false;
  }
}
async function openDesktop(monitor: DesktopMonitor) {
  if (busy.value || !valid.value) return;
  const previous = desktopSession(monitor);
  if (previous) {
    if (!previous.controlled) await resume(previous.session_id, `远端桌面 · ${monitor.name}`);
    return;
  }
  busy.value = true;
  error.value = "";
  notice.value = "";
  const sent = { ...endpoint.value };
  const key = `${signature()}:screen:${monitor.id}`;
  try {
    const result = await invoke<Result>("start_desktop", {
      endpoint: sent, monitorId: monitor.id, monitorName: monitor.name, requestId: attempts.id(key),
      videoFps: videoFPS.value,
    });
    attempts.resolve(key);
    if (signature() === endpointKey(sent)) record(`远端桌面 · ${monitor.name}`, result);
  } catch (e) {
    const failure = launchFailure(e);
    if (!failure.retrySameRequest) attempts.resolve(key);
    if (signature() === endpointKey(sent)) error.value = failure.message;
  } finally {
    await readRuntime(sent);
    busy.value = false;
  }
}
async function resume(sessionId: string, name: string, iconPng?: string | null, applicationKey?: string, appId?: string) {
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
      videoFps: videoFPS.value,
      applicationKey: applicationKey ?? null,
    });
    if (signature() === endpointKey(sent)) record(name, result, appId, applicationKey);
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
  const session = sessions.value.find(s => s.session_id === row.sessionId);
  const changingPresentation = session && session.display === 0 && session.mode !== "desktop" &&
    (session.mode === "scene") !== useApplicationScene(row.appId);
  if (row.sessionId && !changingPresentation) await resume(row.sessionId, row.name, row.icon_png, row.applicationKey, row.appId);
  else await launch({ id: row.appId, name: row.name, icon_png: row.icon_png, application_key: row.applicationKey });
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
watch(signature, () => { catalogView.value = "all"; catalogExpanded.value = false; shortcutWarning.value = ""; });
watch(catalogView, () => { catalogExpanded.value = false; });
watch(activePage, () => window.scrollTo?.(0, 0), { flush: "post" });
onMounted(() => {
  void readPictureMode();
  void readVideoColor();
  window.addEventListener("focus", onFocus);
  window.addEventListener("online", onOnline);
  restoreCatalog();
  if (valid.value) void load();
  void readLocal();
  void readNetwork();
  localTimer=setInterval(()=> {void readLocal(); void readNetwork(); void readAutomatic();},5000);
});
onUnmounted(() => {
  disposed = true; runtimeRevision++; stopRetry(); stopStartupPoll();
  if(localTimer) clearInterval(localTimer);
  if(inviteTimer) clearTimeout(inviteTimer);
  window.removeEventListener("focus", onFocus);
  window.removeEventListener("online", onOnline);
});
</script>
<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand">
        <span class="brand-icon" aria-hidden="true">↗</span><span>窗渡<br /><b>PaneFerry</b></span>
      </div>
      <nav class="navigation" aria-label="主导航">
        <template v-for="(item, index) in navigation" :key="item.id">
          <p v-if="index === 1" class="nav-group-label">管理与偏好</p>
          <button class="nav-item" :class="{ active: activePage === item.id }" :aria-current="activePage === item.id ? 'page' : undefined" @click="activePage = item.id">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="item.icon" /></svg>
            <span>{{ item.label }}</span>
            <span v-if="item.id === 'devices' && nativePeers.length" class="nav-count">{{ nativePeers.length }}</span>
          </button>
        </template>
      </nav>
      <div class="aside-bottom">
        <span class="network-note"><span class="status-dot" />设备间加密连接</span>
        <small>开发预览 0.1.3</small>
      </div>
    </aside>
    <main>
      <header class="page-header">
        <div>
          <div class="eyebrow">{{ currentPage.eyebrow }}</div>
          <h1>{{ currentPage.label }}</h1>
          <p>{{ currentPage.description }}</p>
        </div>
        <div v-if="activePage === 'apps'" class="workspace-search"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5" /><path d="m16 16 5 5" /></svg><input v-model="search" type="search" placeholder="搜索应用或窗口…" aria-label="搜索应用或窗口" /></div>
      </header>

      <section v-if="activePage === 'devices' && (savedConnections.length || savedConnectionError || savedConnectionNotice)" class="panel saved-connections" aria-labelledby="saved-connections-title">
        <div class="section-title"><h2 id="saved-connections-title">已保存连接 <span class="count">{{ savedConnections.length }}</span></h2></div>
        <div class="saved-connection-list">
          <article v-for="row in savedConnections" :key="row.peerId" class="saved-connection-row">
            <div class="window-label"><strong>{{ row.name }}</strong><p>{{ row.address }}</p></div>
            <span v-if="!nativePeers.some(peer => peer.device_id === row.peerId)" class="window-state">{{ networkReady ? "需重新配对" : "正在读取设备" }}</span>
            <span v-else-if="endpoint.peerId === row.peerId && selectedPeer?.address === row.address" class="window-state">当前设备</span>
            <button class="primary-action" :disabled="busy || pairing || !!switchingConnection || !nativePeers.some(peer => peer.device_id === row.peerId)" @click="connectSaved(row)">{{ switchingConnection === row.peerId ? "切换中…" : "切换连接" }}</button>
            <button :disabled="!!switchingConnection" :aria-label="'移除保存的连接：' + row.name" @click="removeSavedConnection(row)">移除</button>
          </article>
        </div>
        <p v-if="savedConnectionError" class="alert error" role="alert">{{ savedConnectionError }}</p>
        <p v-if="savedConnectionNotice" class="hint" role="status">{{ savedConnectionNotice }}</p>
      </section>

      <div v-if="activePage === 'apps'" class="page-content">
        <div class="workspace-toolbar">
          <div class="device-picker">
            <span class="device-symbol" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8m-4-4v4" /></svg></span>
            <div class="device-selection"><label for="workspace-device">当前设备</label>
            <select id="workspace-device" :value="endpoint.peerId" :disabled="busy || pairing || !!switchingConnection || !nativePeers.length" @change="changePeer">
              <option value="" disabled>选择已配对设备</option>
              <option v-if="endpoint.peerId && !selectedPeer" :value="endpoint.peerId">上次使用的设备</option>
              <option v-for="peer in nativePeers" :key="peer.device_id" :value="peer.device_id">{{ peerLabel(peer) }}</option>
            </select>
            </div>
            <span v-if="selectedPeer" class="device-status" :class="{ online: selectedPeer.connected }" role="status"><span class="status-dot" />{{ switchingConnection ? "切换中…" : selectedPeer.connected ? "网络已连接" : "已配对 · 等待连接" }}</span>
            <span v-if="selectedPeer" class="device-address">{{ selectedPeer.address }}</span>
          </div>
          <button class="text-action" @click="activePage = 'devices'">{{ nativePeers.length ? "管理设备" : "添加设备" }} ↗</button>
        </div>
        <p v-if="nativeWarning || savedConnectionError" class="alert error" role="alert">{{ nativeWarning || savedConnectionError }}</p>
        <div v-if="error" class="alert error" role="alert">{{ error }}</div>
        <div v-if="notice" class="alert notice" role="status">{{ notice }}</div>
        <div v-if="valid" class="workspace-actions">
        <section class="quick-action desktop-action" aria-labelledby="desktop-title">
          <div class="quick-action-heading"><span class="action-symbol" aria-hidden="true">▣</span><div><h2 id="desktop-title">整桌面</h2><p class="hint">查看完整桌面和系统弹窗</p></div><button class="icon-action" :disabled="busy" aria-label="刷新显示器" title="刷新显示器" @click="refresh">↻</button></div>
          <p v-if="monitorWarning" class="hint" role="status">{{ monitorWarning }}</p>
          <p v-if="!desktopMonitors.length && !monitorWarning" class="hint">{{ busy ? "正在读取显示器…" : "当前没有可连接的显示器。" }}</p>
          <div v-for="monitor in desktopMonitors" :key="monitor.id" class="monitor-row">
            <div class="window-label"><strong :title="monitor.name">{{ monitor.name }}</strong><p>{{ monitor.width }} × {{ monitor.height }}{{ monitor.primary ? " · 主显示器" : "" }}</p></div>
            <button :disabled="busy || desktopSession(monitor)?.controlled" @click="openDesktop(monitor)">{{ desktopSession(monitor)?.controlled ? "已连接" : "连接桌面" }}</button>
          </div>
        </section>
        <section class="quick-action window-takeover" :class="{ 'takeover-active': automaticStatus.takeoverEnabled }" aria-labelledby="window-takeover-title">
          <div class="quick-action-heading"><span class="action-symbol" aria-hidden="true">▦</span><div><h2 id="window-takeover-title">全窗口接管</h2><p class="hint">自动接入已有窗口和之后的新窗口</p></div></div>
          <div class="takeover-controls">
            <span class="device-status" :class="{ online: automaticStatus.takeoverEnabled }"><span class="status-dot" />{{ automaticStatus.takeoverEnabled ? "正在接管" : "未开启" }}{{ automaticStatus.managedWindows ? ` · ${automaticStatus.managedWindows} 个窗口` : "" }}</span>
            <button :disabled="automaticSaving" :aria-pressed="Boolean(automaticStatus.takeoverEnabled)" @click="deviceWindowAction()">{{ automaticSaving ? "处理中…" : automaticStatus.takeoverEnabled ? "停止接管" : "接管全部窗口" }}</button>
          </div>
          <details class="action-details"><summary>接管说明{{ automaticStatus.managedWindows ? "与窗口管理" : "" }}</summary><p class="hint">菜单、选择器和对话框也会自动接入，已连接窗口直接复用。停止接管会保留已连接窗口并恢复应用级规则；再次开启会重新接入此前关闭的窗口。退出启动器后停止自动接入。</p><button v-if="automaticStatus.managedWindows" class="danger-action" :disabled="automaticSaving" @click="deviceWindowAction(true)">断开此设备全部应用窗口</button></details>
          <p v-if="takeoverError || (automaticStatus.takeoverEnabled && automaticStatus.warning)" class="alert error" role="alert">{{ takeoverError || automaticStatus.warning }}</p>
        </section>
        </div>

        <section v-if="catalogCurrent || runtimeCurrent" class="workspace-windows" aria-labelledby="running-title">
          <div class="section-title"><h2 id="running-title">运行中的窗口 <span class="count">{{ runningConnections.length }}</span><span v-if="runningConnections.some(row => row.controlled)" class="section-note">{{ runningConnections.filter(row => row.controlled).length }} 个已连接</span></h2><button class="text-action" :disabled="busy" @click="refresh">刷新运行状态 ↻</button></div>
          <p v-if="windowWarning" class="alert error" role="status">{{ windowWarning }}</p>
          <p v-if="sessionWarning" class="alert error" role="status">{{ sessionWarning }}</p>
          <p v-if="!runningConnections.length && !windowWarning && !sessionWarning" class="inline-empty">{{ busy ? "正在读取运行状态…" : "暂无运行窗口，从下方打开一个应用即可开始。" }}</p>
          <p v-else-if="runningConnections.length && !filteredConnections.length" class="inline-empty">没有匹配的窗口。</p>
          <div class="running-list">
          <div v-for="row in filteredConnections" :key="row.key" class="desktop-window-row" :class="{ 'window-connected': row.controlled }">
            <AppIcon :icon="row.icon_png" :name="row.name" />
            <div class="window-label"><strong>{{ row.name }}</strong><p :title="row.title">{{ row.title }}</p></div>
            <span class="window-state">{{ row.minimized ? "已最小化" : row.controlled ? "已连接" : "可连接" }}</span>
            <button :disabled="busy || row.controlled" @click="connect(row)">{{ row.controlled ? "已连接" : "连接窗口" }}</button>
          </div>
          </div>
          <details v-if="runningConnections.length" class="action-details"><summary>关闭窗口会怎样？</summary><p class="hint">关闭本机接收窗口只会断开连接，原设备上的应用继续运行。</p></details>
        </section>

        <section v-if="currentStarting.length" class="workspace-windows" aria-live="polite">
          <div class="section-title"><h2>启动中的应用 <span class="count">{{ currentStarting.length }}</span></h2><button class="text-action" :disabled="busy" @click="refresh">检查状态</button></div>
          <p v-if="!filteredStarting.length" class="hint">没有匹配的应用。</p>
          <div v-for="item in filteredStarting" :key="item.endpoint + item.appId" class="desktop-window-row">
            <AppIcon :icon="apps.find(app => app.id === item.appId)?.icon_png" :name="item.name" />
            <div class="window-label"><strong>{{ item.name }}</strong><p>正在等待应用创建窗口，无需重复打开</p></div><span class="window-state">启动中…</span>
          </div>
        </section>

        <section v-if="quickApps.length" class="quick-apps" aria-labelledby="quick-apps-title">
          <div class="section-title"><h2 id="quick-apps-title">常用与最近</h2><span class="subtle-label">仅当前设备</span></div>
          <div class="quick-app-grid"><button v-for="app in quickApps" :key="app.id" class="quick-app" :title="app.unavailable_reason || app.name" :disabled="busy || !catalogCurrent || Boolean(app.unavailable_reason) || isStarting(app.id)" @click="launch(app)"><AppIcon :icon="app.icon_png" :name="app.name" /><span>{{ app.name }}</span><span class="quick-app-mark" aria-hidden="true">{{ favoriteIds.has(app.id) ? "★" : "↗" }}</span></button></div>
        </section>

        <section class="catalog application-catalog" aria-labelledby="catalog-title">
          <div class="section-title">
            <h2 id="catalog-title">应用 <span class="count">{{ apps.filter(app => app.source === "desktop").length }}</span></h2>
            <div class="catalog-actions">
              <span v-if="catalogSavedAt" class="catalog-timestamp" :title="new Date(catalogSavedAt).toLocaleString()">已缓存列表</span>
              <button class="text-action" :disabled="!valid || busy" @click="load(true)">{{ busy ? "读取中…" : "刷新应用 ↻" }}</button>
            </div>
          </div>
          <div class="catalog-filters" role="group" aria-label="应用列表筛选"><button :class="{ selected: catalogView === 'all' }" :aria-pressed="catalogView === 'all'" @click="catalogView = 'all'">全部应用</button><button :class="{ selected: catalogView === 'favorites' }" :aria-pressed="catalogView === 'favorites'" @click="catalogView = 'favorites'">收藏 <span>{{ favoriteApps.length }}</span></button><button :class="{ selected: catalogView === 'recent' }" :aria-pressed="catalogView === 'recent'" @click="catalogView = 'recent'">最近使用</button><span class="catalog-tip">点星号收藏，常用应用会出现在上方</span></div>
          <p v-if="catalogSavedAt && !catalogPersisted" class="hint">本机缓存无法保存；当前列表仍可使用，退出启动器后需要重新读取。</p>
          <p v-if="discoveryWarning" class="alert error" role="status">{{ discoveryWarning }}</p>
          <p v-if="shortcutWarning" class="alert error" role="status">{{ shortcutWarning }}</p>
          <div v-if="!apps.some(app => app.source === 'desktop')" class="empty">
            <div class="empty-icon" aria-hidden="true">▦</div>
            <h3>{{ busy ? "正在读取设备应用" : valid ? "这台设备暂无可显示的应用" : "从另一台设备开始" }}</h3>
            <p>{{ valid ? "可刷新应用列表，或检查对方设备是否已开启共享。" : "选择一台已配对设备，即可在这里打开它的应用。" }}</p>
            <button v-if="!valid" class="primary-action" @click="activePage = 'devices'">{{ nativePeers.length ? "选择设备" : "配对第一台设备" }}</button>
          </div>
          <div v-else-if="catalogApps.length" class="app-grid compact-app-grid">
            <article v-for="app in visibleApps" :key="app.id" class="app-tile">
            <button class="app-card" :title="app.unavailable_reason || app.name" :disabled="busy || !catalogCurrent || Boolean(app.unavailable_reason) || isStarting(app.id)" @click="launch(app)">
              <AppIcon :icon="app.icon_png" :name="app.name" />
              <span class="app-card-label"><strong>{{ app.name }}</strong><span class="app-description">{{ app.unavailable_reason || (isStarting(app.id) || launchingAppId === app.id ? "启动中…" : app.description || "打开应用") }}</span></span>
            </button><button class="favorite-action" :class="{ favorited: favoriteIds.has(app.id) }" :aria-pressed="favoriteIds.has(app.id)" :aria-label="`${favoriteIds.has(app.id) ? '取消收藏' : '收藏'} ${app.name}`" :title="favoriteIds.has(app.id) ? '取消收藏' : '收藏应用'" @click="toggleFavorite(app)">{{ favoriteIds.has(app.id) ? "★" : "☆" }}</button>
            </article>
          </div>
          <div v-else class="inline-empty">{{ search ? "没有匹配的应用，试试其他关键词。" : catalogView === "favorites" ? "还没有收藏。在全部应用中点击星号，即可固定常用入口。" : "打开或连接应用后，它会出现在这里。" }}</div>
          <button v-if="catalogApps.length > 12 && !search.trim()" class="expand-catalog" :aria-expanded="catalogExpanded" @click="catalogExpanded = !catalogExpanded">{{ catalogExpanded ? "收起列表 ↑" : `显示全部 ${catalogApps.length} 个应用 ↓` }}</button>
          <p v-if="apps.length && !catalogCurrent" class="hint">设备已改变，请刷新当前设备的应用。</p>
        </section>
        <details v-if="valid" class="panel automatic-windows"><summary class="settings-summary"><span>新窗口自动接入</span><span class="subtle-label">按应用配置 · 更改自动生效</span></summary>
          <p v-if="automaticStatus.takeoverEnabled" class="hint" role="status">全窗口接管已开启；以下应用级规则保留，停止接管后恢复生效。</p>
          <fieldset v-if="automaticOptions.length" class="automatic-applications" :disabled="automaticSaving || automaticStatus.takeoverEnabled">
          <legend>按应用配置新窗口接入</legend>
          <p class="hint">应用连接期间，按它的规则接入远端新窗口。多个应用可以同时开启，同一窗口只显示一次，手动连接不会重复打开。</p>
          <div class="automatic-fields">
            <label for="automatic-app">配置哪个应用
              <select id="automatic-app" :value="automaticKey" :disabled="automaticSaving" @change="automaticApp = ($event.target as HTMLSelectElement).value">
                <option v-for="app in automaticOptions" :key="app.id" :value="app.id">{{ app.name }}{{ app.connected ? " · 已连接" : app.unavailableReason ? " · 入口不可用" : "" }}</option>
              </select>
            </label>
            <label for="automatic-mode">该应用连接时
              <select id="automatic-mode" :value="automaticRule.mode" :disabled="automaticSaving" @change="changeAutomaticMode">
                <option value="off">不自动接入</option><option value="all" :disabled="Boolean(automaticUnavailable)">接入全部新窗口</option><option value="selected" :disabled="Boolean(automaticUnavailable)">仅接入指定应用的新窗口</option>
              </select>
            </label>
          </div>
          <p v-if="automaticUnavailable" class="hint" role="status">此入口当前不可用，规则不会生效。{{ automaticUnavailable }} 请选择正在连接的应用；旧规则仍可关闭。</p>
          <fieldset v-if="automaticRule.mode === 'selected'" class="automatic-applications" :disabled="automaticSaving || Boolean(automaticUnavailable)">
            <legend>允许自动接入的应用</legend>
            <p class="hint">按窗口实际所属应用匹配。系统文件选择器等独立来源也需要加入名单。</p>
            <label v-for="app in automaticOptions" :key="app.id">
              <input type="checkbox" :checked="automaticRule.applications.includes(app.id)" @change="changeAutomaticApplication(app.id, $event)" />{{ app.name }}
            </label>
          </fieldset>
          <p v-if="!automaticUnavailable" class="hint" role="status">{{ automaticSaving ? "正在保存…" : automaticRule.mode === "off" ? "此应用未开启自动接入。" : automaticStatus.connectedApplications.includes(automaticKey) ? "此应用已连接，规则在后台生效。" : "设置已保存，连接此应用后生效。" }}</p>
          <ul v-if="automaticStatus.rules.some(item => item.rule.mode !== 'off')" class="automatic-enabled">
            <li v-for="item in automaticStatus.rules.filter(item => item.rule.mode !== 'off')" :key="item.applicationKey">
              {{ item.name }} · {{ item.rule.mode === "all" ? "全部新窗口" : `指定 ${item.rule.applications.length} 个应用` }} · {{ automaticStatus.connectedApplications.includes(item.applicationKey) ? "已连接" : automaticOptions.find(app => app.id === item.applicationKey)?.unavailableReason ? "入口不可用" : "待连接" }}
            </li>
          </ul>
          </fieldset>
          <p class="hint">应用级规则仅接入启用后新出现的窗口。关闭接收窗口后冷却 5 秒；仍然存在的同一个窗口不会反复弹回，随时可以手动重连。退出启动器后停止自动接入。</p>
          <p v-if="automaticError || (!automaticStatus.takeoverEnabled && automaticStatus.warning)" class="alert error" role="alert">{{ automaticError || automaticStatus.warning }}</p>
        </details>
        <section v-if="custom.length" class="catalog">
          <div class="section-title"><h2>自定义入口 <span class="count">{{ custom.length }}</span></h2></div>
          <div class="app-grid">
            <button v-for="app in custom" :key="app.id" class="app-card" :disabled="busy || !catalogCurrent || Boolean(app.unavailable_reason) || isStarting(app.id)" @click="launch(app)">
              <AppIcon :icon="app.icon_png" :name="app.name" /><strong>{{ app.name }}</strong>
              <p :title="app.unavailable_reason || app.description || app.id">{{ app.unavailable_reason || app.description || app.id }}</p>
              <span class="launch-label">{{ app.unavailable_reason ? "入口不可用" : isStarting(app.id) || launchingAppId === app.id ? "启动中…" : "打开应用 ↗" }}</span>
            </button>
          </div>
        </section>
      </div>

      <div v-else-if="activePage === 'devices'" class="page-content">
        <p v-if="nativeWarning" class="alert error" role="alert">{{ nativeWarning }}</p>
        <p v-if="pairNotice" class="alert notice" role="status">{{ pairNotice }}</p>
        <section class="panel">
          <div class="section-title"><h2>已配对设备 <span class="count">{{ nativePeers.length }}</span></h2></div>
          <p v-if="!nativePeers.length" class="hint">还没有配对设备。在下方添加设备，或生成邀请让另一台设备连接本机。</p>
          <article v-for="peer in nativePeers" :key="peer.device_id" class="peer-card">
            <div class="peer-heading">
              <div class="peer-identity"><span class="device-icon" aria-hidden="true">↔</span><div class="window-label"><h3>{{ peer.name }}</h3><p>{{ peer.address }}</p></div></div>
              <span class="device-status" :class="{ online: peer.connected }"><span class="status-dot" />{{ peer.connected ? "已连接" : "已配对" }}</span>
              <button class="primary-action" :disabled="busy || pairing" @click="selectPeer(peer)">查看应用 ↗</button>
            </div>
            <div class="peer-address">
              <label :for="'connection-name-' + peer.device_id">连接名称<input :id="'connection-name-' + peer.device_id" v-model="connectionNames[peer.device_id]" maxlength="80" :disabled="pairing" placeholder="例如：家里的 Windows" /></label>
              <label :for="'address-' + peer.device_id">连接地址<input :id="'address-' + peer.device_id" v-model="addressEdits[peer.device_id]" :disabled="pairing" spellcheck="false" /></label>
              <button :disabled="pairing" @click="savePeerConnection(peer)">保存连接</button>
              <button :disabled="pairing" @click="updatePeerAddress(peer)">更新地址</button>
              <button class="danger-action" :disabled="pairing" @click="revokePeer(peer)">撤销配对</button>
            </div>
            <p class="hint">每台设备保存一项；再次保存会更新名称和地址。点击“切换连接”时使用保存的地址，已打开的远端窗口继续保留。</p>
            <p class="fingerprint" :title="peer.fingerprint">设备指纹 <code>{{ peer.fingerprint }}</code></p>
            <div v-if="localNode?.platform === 'macos' || localNode?.platform === 'windows'" class="setting-row">
              <div><label :for="'audio-' + peer.device_id">接收这台设备的系统声音</label><p class="hint">默认关闭。需要新版 Windows 发布端；接收所有应用播放的声音，不采集麦克风。多个窗口只播放一份；关闭画面后仍可收听，关闭此开关停止。</p><p class="hint" role="status">{{ audioLabel(peer.device_id) }}</p></div>
              <input :id="'audio-' + peer.device_id" type="checkbox" :checked="deviceAudio[peer.device_id]?.enabled ?? false" :disabled="audioSaving[peer.device_id] || pairing" @change="setAudio(peer.device_id, $event)" />
            </div>
            <p v-if="audioErrors[peer.device_id] || deviceAudio[peer.device_id]?.message" class="hint" role="status">{{ audioErrors[peer.device_id] || deviceAudio[peer.device_id]?.message }}</p>
          </article>
        </section>
        <section class="panel">
          <div class="section-title"><h2>添加设备</h2><button :disabled="pairing" @click="inviteDevice">生成本机邀请</button></div>
          <p class="hint">两端打开 窗渡 PaneFerry，填写对方的局域网地址和配对邀请。</p>
          <form class="fields" @submit.prevent="pairDevice">
            <label for="pair-address">对方地址<input id="pair-address" v-model="pairAddress" placeholder="192.168.1.20:42424" :disabled="pairing" spellcheck="false" /></label>
            <label for="pair-code">对方邀请<input id="pair-code" v-model="pairCode" type="password" autocomplete="off" placeholder="粘贴对方生成的邀请" :disabled="pairing" /></label>
            <button class="primary-action" :disabled="pairing || !pairAddress.trim() || !pairCode.trim()" type="submit">配对设备</button>
          </form>
          <div v-if="ownInvite" class="invite-box">
            <label for="own-invite">本机配对邀请</label>
            <textarea id="own-invite" :value="ownInvite" readonly rows="3" spellcheck="false" />
            <p class="hint">将这段邀请填写到对方设备，并使用本机的局域网地址连接。</p>
          </div>
          <p class="hint">邀请五分钟内有效、仅可使用一次，请通过可信方式传递。</p>
        </section>
      </div>

      <div v-else-if="activePage === 'sharing'" class="page-content">
        <section class="panel">
          <div class="section-title"><h2>本机应用共享</h2><span class="badge" :class="{ enabled: localNode?.sharing_enabled }">{{ localNode ? localNode.sharing_enabled ? "已开启" : "未开启" : "正在读取…" }}</span></div>
          <div class="sharing-summary">
            <div><h3>{{ localNode?.name || "这台设备" }}</h3><p class="hint">{{ localNode?.sharing_enabled ? "已配对设备可以读取并打开本机应用。" : "开启后，已配对设备可以读取并打开本机应用。" }}</p></div>
            <button :class="{ 'primary-action': !localNode?.sharing_enabled }" :disabled="localBusy || !localNode" @click="toggleSharing">{{ localNode?.sharing_enabled ? "停止共享" : "开启共享" }}</button>
          </div>
          <p class="hint separated">应用仍在本机运行并显示。收到的远程应用不会再次共享给其他设备。</p>
          <p v-if="localWarning" class="alert error" role="alert">{{ localWarning }}</p>
        </section>
        <section v-if="localNode?.platform === 'windows'" class="panel">
          <div class="section-title"><h2>Windows 当前桌面</h2></div>
          <p class="hint">共享当前用户的应用窗口或所选显示器。请保持桌面登录并解锁；管理员程序和 UAC 安全桌面的操作暂不支持。本机和远端共用鼠标、键盘与焦点。</p>
        </section>
        <section v-else-if="localNode?.permissions" class="panel">
          <div class="section-title"><h2>系统权限</h2></div>
          <div class="setting-row"><div><h3>屏幕录制</h3><p class="hint">用于传输本机应用窗口的画面。</p></div><span class="permission-state" :class="{ granted: localNode.permissions.screen_recording }">{{ localNode.permissions.screen_recording ? "已允许" : "待允许" }}</span></div>
          <div class="setting-row"><div><h3>辅助功能</h3><p class="hint">用于接收远程键盘和鼠标操作。</p></div><span class="permission-state" :class="{ granted: localNode.permissions.accessibility }">{{ localNode.permissions.accessibility ? "已允许" : "待允许" }}</span></div>
          <template v-if="!localNode.permissions.screen_recording || !localNode.permissions.accessibility">
            <div class="button-row"><button class="primary-action" :disabled="localBusy" @click="requestPermissions">允许共享所需权限</button><button :disabled="localBusy" @click="showLocalComponent">显示共享组件</button></div>
            <p class="hint">系统列表已开启仍不生效时，点“显示共享组件”，移除权限列表中的旧条目，再添加访达选中的当前程序。</p>
          </template>
        </section>
      </div>

      <div v-else-if="activePage === 'display'" class="page-content">
        <section class="panel">
          <div class="section-title"><h2>窗口显示</h2><span class="subtle-label">更改自动保存</span></div>
          <div class="setting-row"><div><label for="video-fps">默认帧率</label><p class="hint">60 帧更流畅，30 帧减少传输和处理负担。实际帧率随画面内容变化。</p><span class="setting-effect">新开或重新连接后生效</span></div><select id="video-fps" v-model.number="videoFPS" :disabled="busy" @change="save"><option :value="30">30 帧/秒</option><option :value="60">60 帧/秒</option></select></div>
          <p class="hint">已打开窗口可在它的“显示 → 帧率”菜单中即时切换。</p>
          <div class="setting-row"><div><label for="picture-mode">画面尺寸</label><p class="hint">适配本机：远端窗口跟随本机内容区调整。保持源尺寸：只缩放本机显示，远端窗口大小保持不变。</p><span class="setting-effect">关闭接收窗口并重新接管后生效</span></div><select id="picture-mode" :value="pictureMode" :disabled="!pictureModeReady || pictureModeSaving || busy" @change="savePictureMode"><option value="adapt">适配本机窗口</option><option value="source">保持源窗口尺寸</option></select></div>
          <p v-if="pictureModeError" role="alert">{{ pictureModeError }}</p>
        </section>
        <details class="panel advanced-settings"><summary class="settings-summary"><span>高级显示设置</span><span class="subtle-label">{{ videoColor === "444-atlas-v1" ? "清晰色彩" : "兼容色彩" }} · 会话画面{{ applicationScene && pictureMode !== "source" ? "开启" : "关闭" }}</span></summary>
          <div v-if="localNode?.platform === 'macos'" class="setting-row"><div><label for="video-color">接收色彩</label><p class="hint">清晰色彩保留彩色文字和细线的细节，需要支持此功能的发布端，带宽及编解码负担更高。不支持时使用兼容模式；两种模式均为有损视频。</p><span class="setting-effect">关闭接收窗口并重新接管后生效</span></div><select id="video-color" :value="videoColor" :disabled="!videoColorReady || videoColorSaving || busy" @change="saveVideoColor"><option value="420">兼容 · 4:2:0</option><option value="444-atlas-v1">清晰色彩 · 4:4:4（试用）</option></select></div>
          <p v-if="videoColorError" role="alert">{{ videoColorError }}</p>
          <div class="setting-row"><div><label for="application-scene">应用会话画面 · 试用</label><p class="hint">将受支持应用的主窗口、菜单和对话框合并显示。画布按连接时的屏幕大小确定，本机调整窗口时等比显示；其他来源使用兼容显示。</p><p v-if="pictureMode === 'source'" class="hint">使用“适配本机窗口”后可开启此模式。</p><span class="setting-effect">下次连接应用时生效</span></div><input id="application-scene" type="checkbox" v-model="applicationScene" :disabled="busy || pictureMode === 'source'" @change="save" /></div>
        </details>
        <section class="panel">
          <div class="section-title"><h2>输入与剪贴板</h2></div>
          <div class="setting-row"><div><h3>输入法</h3><p class="hint">使用当前控制设备的输入法。</p></div></div>
          <div class="setting-row"><div><h3>复制与粘贴</h3><p class="hint">主动复制或粘贴时传输剪贴板内容，不做后台同步。</p></div></div>
        </section>
      </div>

      <div v-else-if="activePage === 'history'" class="page-content">
        <section class="panel history">
          <div class="section-title"><h2>本次连接记录 <span class="count">{{ launches.length }}</span></h2><span class="subtle-label">最近 20 次</span></div>
          <div v-if="!launches.length" class="empty">
            <div class="empty-icon" aria-hidden="true">↗</div><h3>还没有连接记录</h3><p>从工作台打开应用或连接已有窗口后，记录会显示在这里。</p><button @click="activePage = 'apps'">前往应用工作台</button>
          </div>
          <ol v-else class="history-list">
            <li v-for="(entry, i) in launches" :key="i">
              <div class="history-heading"><h3>{{ entry.name }}</h3><time :datetime="new Date(entry.createdAt).toISOString()">{{ new Date(entry.createdAt).toLocaleTimeString() }}</time></div>
              <p class="hint" :title="entry.peerId">{{ entry.deviceName }}{{ entry.deviceAddress ? " · " + entry.deviceAddress : "" }} · 已请求打开原生窗口</p>
              <code v-if="entry.result.session_id" class="session-id">会话 {{ entry.result.session_id }}</code>
            </li>
          </ol>
          <p v-if="launches.length" class="hint">请以实际应用窗口确认连接成功。</p>
        </section>
      </div>
      <footer>一套软件 · 双向应用共享<span>窗渡 PaneFerry</span></footer>
    </main>
  </div>
</template>
