mod policy;
use crate::{
    network::{self, Device},
    LaunchResult,
};
use paneferry_transport::{self as transport, DesktopWindowInfo};
pub use policy::{Mode, Rule};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::Manager;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preference {
    pub peer_id: String,
    pub application_key: String,
    pub name: String,
    pub rule: Rule,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DevicePreference {
    peer_id: String,
    enabled: bool,
    video_fps: u8,
}
impl DevicePreference {
    fn validate(&self) -> Result<(), String> {
        Device {
            peer_id: self.peer_id.clone(),
        }
        .validate()
        .map_err(|e| e.to_string())?;
        crate::requested_video_fps(Some(self.video_fps)).map_err(|e| e.to_string())?;
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    rules: Vec<Preference>,
    devices: Vec<DevicePreference>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredSettings {
    Legacy(Vec<Preference>),
    Current(Settings),
}
impl Preference {
    fn validate(&self) -> Result<(), String> {
        Device {
            peer_id: self.peer_id.clone(),
        }
        .validate()
        .map_err(|e| e.to_string())?;
        transport::validate_id(&self.application_key).map_err(|e| e.to_string())?;
        if self.name.is_empty() || self.name.len() > 1024 {
            return Err("应用名称无效。".into());
        }
        self.rule.validate()
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub rules: Vec<Preference>,
    pub connected_applications: Vec<String>,
    pub warning: String,
    pub takeover_enabled: bool,
    pub managed_windows: usize,
}
#[derive(Clone)]
pub struct Source {
    pub application_key: String,
    pub target_id: String,
}
#[derive(Clone)]
struct Host {
    source: Source,
    pid: u32,
    registration: u64,
    fps: u8,
    stop: Option<std::sync::mpsc::Sender<()>>,
}
#[derive(Default)]
struct Data {
    planner: policy::Planner,
    hosts: HashMap<String, Host>,
    windows: Vec<DesktopWindowInfo>,
    manual: HashMap<String, String>,
    waiting: usize,
    revision: u64,
    connected: HashSet<String>,
    had_connection: bool,
    warning: String,
    connection_failures: BTreeMap<String, String>,
    disconnecting: bool,
}
impl Data {
    fn refresh_connection_failures(&mut self, windows: &[DesktopWindowInfo]) {
        self.connection_failures
            .retain(|id, _| windows.iter().any(|w| &w.id == id && !w.controlled));
    }
}
pub struct Peer {
    pub gate: tokio::sync::Mutex<()>,
    data: Mutex<Data>,
    clock: Instant,
}
impl Peer {
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }
    pub fn manual(self: &Arc<Self>, request: &str, target: &str) -> Manual {
        let mut data = self.data.lock().unwrap();
        data.waiting += 1;
        data.revision += 1;
        data.manual.insert(request.into(), target.into());
        Manual {
            peer: self.clone(),
            request: request.into(),
        }
    }
    pub fn existing(&self, target: &str, session: Option<&str>) -> Option<LaunchResult> {
        let data = self.data.lock().unwrap();
        let mut matches = data.hosts.iter().filter(|(id, host)| {
            session.map_or_else(
                || {
                    host.source.target_id == target
                        || data
                            .windows
                            .iter()
                            .any(|w| w.id == target && w.session_id.as_deref() == Some(id.as_str()))
                        || !target.starts_with("window-")
                            && !target.starts_with("wayland-")
                            && host.source.application_key == target
                },
                |session| session == id.as_str(),
            )
        });
        let (id, host) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(LaunchResult {
            session_id: Some(id.clone()),
            display: None,
            pid: host.pid,
            message: "该窗口已连接，继续使用现有接收窗口。".into(),
        })
    }
    pub fn register(&self, session: &str, source: Source, pid: u32, fps: u8) -> u64 {
        let mut data = self.data.lock().unwrap();
        data.revision += 1;
        let registration = data.revision;
        data.hosts.insert(
            session.into(),
            Host {
                source,
                pid,
                registration,
                fps,
                stop: None,
            },
        );
        registration
    }
    pub fn set_stop(&self, session: &str, registration: u64, stop: std::sync::mpsc::Sender<()>) {
        if let Some(host) = self
            .data
            .lock()
            .unwrap()
            .hosts
            .get_mut(session)
            .filter(|h| h.registration == registration)
        {
            host.stop = Some(stop);
        }
    }
    fn disconnect_all(&self) {
        let mut data = self.data.lock().unwrap();
        data.disconnecting = !data.hosts.is_empty();
        data.revision += 1;
        for host in data.hosts.values() {
            if let Some(stop) = &host.stop {
                let _ = stop.send(());
            }
        }
    }
    pub fn closed(&self, session: &str, pid: u32, registration: u64) {
        let mut data = self.data.lock().unwrap();
        if !data
            .hosts
            .get(session)
            .is_some_and(|h| h.pid == pid && h.registration == registration)
        {
            return;
        }
        let host = data.hosts.remove(session).unwrap();
        data.revision += 1;
        data.connected.remove(session);
        let mut ids: Vec<_> = data
            .windows
            .iter()
            .filter(|w| w.session_id.as_deref() == Some(session))
            .map(|w| w.id.clone())
            .collect();
        ids.push(host.source.target_id);
        data.planner.dismiss(&ids, session, self.now());
    }
    pub async fn baseline(&self, app: &tauri::AppHandle, endpoint: &Device) {
        if self.data.lock().unwrap().planner.armed {
            return;
        }
        if let Ok(target) = network::target(app, endpoint) {
            if let Ok(snapshot) = transport::desktop_windows(&target).await {
                if snapshot.unavailable_reason.is_none() {
                    let mut data = self.data.lock().unwrap();
                    data.planner.baseline(&snapshot.windows);
                    data.windows = snapshot.windows;
                }
            }
        }
    }
}
pub struct Manual {
    peer: Arc<Peer>,
    request: String,
}
impl Manual {
    pub fn resolve(&self) {
        self.peer.data.lock().unwrap().manual.remove(&self.request);
    }
}
impl Drop for Manual {
    fn drop(&mut self) {
        // An interrupted/unknown launch keeps its target reserved until the
        // same idempotent request is resolved. It cannot be stolen by a scan.
        self.peer.data.lock().unwrap().waiting -= 1;
    }
}

pub struct Service {
    path: PathBuf,
    settings: Mutex<Settings>,
    peers: Mutex<HashMap<String, Arc<Peer>>>,
    settings_warning: Mutex<String>,
}
impl Service {
    pub fn start(&self, app: &tauri::AppHandle) {
        let devices = self.settings.lock().unwrap().devices.clone();
        for device in devices.into_iter().filter(|d| d.enabled) {
            self.peer(
                app,
                &Device {
                    peer_id: device.peer_id,
                },
            );
        }
    }
    fn takeover(&self, peer_id: &str) -> Option<DevicePreference> {
        self.settings
            .lock()
            .unwrap()
            .devices
            .iter()
            .find(|d| d.peer_id == peer_id && d.enabled)
            .cloned()
    }
    pub fn configured_for(&self, peer_id: &str, application_key: &str) -> bool {
        self.settings.lock().unwrap().rules.iter().any(|p| {
            p.peer_id == peer_id && p.application_key == application_key && p.rule.mode != Mode::Off
        })
    }
    pub async fn prepare_baseline(&self, app: &tauri::AppHandle, endpoint: &Device, peer: &Peer) {
        {
            let mut data = peer.data.lock().unwrap();
            if self.active_rules(&endpoint.peer_id, &data).is_empty() {
                data.planner.disarm();
            }
        }
        peer.baseline(app, endpoint).await;
    }
    pub fn open(path: PathBuf) -> Self {
        Self::load(path.clone()).unwrap_or_else(|error| Self {
            path,
            settings: Mutex::new(Settings::default()),
            peers: Mutex::new(HashMap::new()),
            settings_warning: Mutex::new(format!("{error} 自动接入已停用，请重新保存设置。")),
        })
    }
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let settings = if path.exists() {
            if path.is_symlink()
                || fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024
            {
                return Err("自动接入设置文件无效。".into());
            }
            let stored: StoredSettings =
                serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|_| "自动接入设置无法读取。")?;
            let settings = match stored {
                StoredSettings::Legacy(rules) => Settings {
                    rules,
                    devices: vec![],
                },
                StoredSettings::Current(settings) => settings,
            };
            if settings.rules.len() > 256 || settings.devices.len() > 64 {
                return Err("自动接入设置数量超限。".into());
            }
            let mut identities = HashSet::new();
            for rule in &settings.rules {
                rule.validate()?;
                if !identities.insert((&rule.peer_id, &rule.application_key)) {
                    return Err("自动接入应用设置重复。".into());
                }
            }
            let mut devices = HashSet::new();
            for device in &settings.devices {
                device.validate()?;
                if !devices.insert(&device.peer_id) {
                    return Err("全窗口接管设置重复。".into());
                }
            }
            settings
        } else {
            Settings::default()
        };
        Ok(Self {
            path,
            settings: Mutex::new(settings),
            peers: Mutex::new(HashMap::new()),
            settings_warning: Mutex::new(String::new()),
        })
    }
    pub fn peer(&self, app: &tauri::AppHandle, endpoint: &Device) -> Arc<Peer> {
        let mut peers = self.peers.lock().unwrap();
        if let Some(peer) = peers.get(&endpoint.peer_id) {
            return peer.clone();
        }
        let peer = Arc::new(Peer {
            gate: tokio::sync::Mutex::new(()),
            data: Mutex::new(Data::default()),
            clock: Instant::now(),
        });
        peers.insert(endpoint.peer_id.clone(), peer.clone());
        let (handle, device, worker) = (app.clone(), endpoint.clone(), peer.clone());
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tick(&handle, &device, &worker).await;
            }
        });
        peer
    }
    fn active_rules(&self, peer_id: &str, data: &Data) -> Vec<Rule> {
        self.rules_for(peer_id, data, false)
    }
    fn rules_for(&self, peer_id: &str, data: &Data, connected_only: bool) -> Vec<Rule> {
        if data.disconnecting {
            return vec![];
        }
        if self.takeover(peer_id).is_some() {
            return vec![Rule {
                mode: Mode::All,
                applications: vec![],
            }];
        }
        self.settings
            .lock()
            .unwrap()
            .rules
            .iter()
            .filter(|p| {
                p.peer_id == peer_id
                    && p.rule.mode != Mode::Off
                    && data.hosts.iter().any(|(id, h)| {
                        h.source.application_key == p.application_key
                            && (!connected_only || data.connected.contains(id))
                    })
            })
            .map(|p| p.rule.clone())
            .collect()
    }
    pub fn status(&self, peer_id: &str) -> Status {
        let takeover_enabled = self.takeover(peer_id).is_some();
        let rules = self
            .settings
            .lock()
            .unwrap()
            .rules
            .iter()
            .filter(|p| p.peer_id == peer_id)
            .cloned()
            .collect();
        let peers = self.peers.lock().unwrap();
        let Some(peer) = peers.get(peer_id) else {
            return Status {
                rules,
                connected_applications: vec![],
                warning: self.settings_warning.lock().unwrap().clone(),
                takeover_enabled,
                managed_windows: 0,
            };
        };
        let data = peer.data.lock().unwrap();
        let mut connected: Vec<_> = data
            .hosts
            .iter()
            .filter(|(id, _)| data.connected.contains(*id))
            .map(|(_, h)| h.source.application_key.clone())
            .collect();
        connected.sort();
        connected.dedup();
        Status {
            rules,
            takeover_enabled,
            managed_windows: data.hosts.len(),
            connected_applications: connected,
            warning: if !data.warning.is_empty() {
                data.warning.clone()
            } else if let Some(message) = data.connection_failures.values().next() {
                message.clone()
            } else {
                self.settings_warning.lock().unwrap().clone()
            },
        }
    }
    fn save(&self, preference: Preference) -> Result<(), String> {
        preference.validate()?;
        self.update(|next| {
            next.rules.retain(|p| {
                p.peer_id != preference.peer_id || p.application_key != preference.application_key
            });
            next.rules.push(preference);
        })
    }
    fn save_device(&self, device: DevicePreference) -> Result<(), String> {
        device.validate()?;
        self.update(|next| {
            next.devices.retain(|d| d.peer_id != device.peer_id);
            next.devices.push(device);
        })
    }
    fn update(&self, update: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut current = self.settings.lock().unwrap();
        let mut next = current.clone();
        update(&mut next);
        if next.rules.len() > 256 || next.devices.len() > 64 {
            return Err("自动接入设置已达到上限。".into());
        }
        let parent = self.path.parent().ok_or("设置目录无效。")?;
        fs::create_dir_all(parent).map_err(|_| "无法创建设置目录。")?;
        let temp = parent.join(format!(".auto-connect-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temp)?;
            file.write_all(&serde_json::to_vec(&next)?)?;
            file.sync_all()?;
            fs::rename(&temp, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
            return Err("自动接入设置保存失败。".into());
        }
        *current = next;
        self.settings_warning.lock().unwrap().clear();
        Ok(())
    }
}

#[tauri::command]
pub fn automatic_window_status(app: tauri::AppHandle, endpoint: Device) -> Result<Status, String> {
    endpoint.validate().map_err(|e| e.to_string())?;
    Ok(app.state::<Service>().status(&endpoint.peer_id))
}

#[tauri::command]
pub async fn set_device_window_takeover(
    app: tauri::AppHandle,
    endpoint: Device,
    enabled: bool,
    video_fps: u8,
) -> Result<Status, String> {
    endpoint.validate().map_err(|e| e.to_string())?;
    let service = app.state::<Service>();
    let peer = service.peer(&app, &endpoint);
    let _gate = peer.gate.lock().await;
    service.save_device(DevicePreference {
        peer_id: endpoint.peer_id.clone(),
        enabled,
        video_fps,
    })?;
    {
        let mut data = peer.data.lock().unwrap();
        data.planner.set_takeover(enabled);
        data.revision += 1;
    }
    Ok(service.status(&endpoint.peer_id))
}

#[tauri::command]
pub async fn disconnect_device_windows(
    app: tauri::AppHandle,
    endpoint: Device,
) -> Result<Status, String> {
    endpoint.validate().map_err(|e| e.to_string())?;
    let service = app.state::<Service>();
    let peer = service.peer(&app, &endpoint);
    let _gate = peer.gate.lock().await;
    service.save_device(DevicePreference {
        peer_id: endpoint.peer_id.clone(),
        enabled: false,
        video_fps: 30,
    })?;
    peer.disconnect_all();
    Ok(service.status(&endpoint.peer_id))
}
#[tauri::command]
pub async fn set_automatic_window_rule(
    app: tauri::AppHandle,
    preference: Preference,
) -> Result<Status, String> {
    preference.validate()?;
    let endpoint = Device {
        peer_id: preference.peer_id.clone(),
    };
    let peer = app.state::<Service>().peer(&app, &endpoint);
    let _gate = peer.gate.lock().await;
    // Establish the old-window boundary before turning the first rule on.
    if preference.rule.mode != Mode::Off {
        app.state::<Service>()
            .prepare_baseline(&app, &endpoint, &peer)
            .await;
    }
    app.state::<Service>().save(preference)?;
    peer.data.lock().unwrap().revision += 1;
    Ok(app.state::<Service>().status(&endpoint.peer_id))
}

async fn tick(app: &tauri::AppHandle, endpoint: &Device, peer: &Arc<Peer>) {
    let service = app.state::<Service>();
    let revision = {
        let mut data = peer.data.lock().unwrap();
        data.planner
            .set_takeover(service.takeover(&endpoint.peer_id).is_some());
        if data.disconnecting && data.hosts.is_empty() {
            data.disconnecting = false;
        }
        if service.active_rules(&endpoint.peer_id, &data).is_empty() {
            data.planner.disarm();
            data.had_connection = false;
            data.warning.clear();
            data.connection_failures.clear();
            return;
        }
        if data.waiting > 0 {
            return;
        }
        data.revision
    };
    let target = match network::target(app, endpoint) {
        Ok(target) => target,
        Err(error) => {
            peer.data.lock().unwrap().warning = error.to_string();
            return;
        }
    };
    let (snapshot, sessions) = tokio::join!(
        transport::desktop_windows(&target),
        transport::sessions(&target)
    );
    let (snapshot, sessions) = match (snapshot, sessions) {
        (Ok(w), Ok(s)) if w.unavailable_reason.is_none() => (w, s),
        _ => {
            peer.data.lock().unwrap().warning = "自动接入暂时无法读取远端窗口，正在重试。".into();
            return;
        }
    };
    // A slow inventory must not hold up an explicit Open action. Discard a
    // snapshot if a manual launch or host lifecycle change raced with it.
    let Ok(_gate) = peer.gate.try_lock() else {
        return;
    };
    let candidate = {
        let mut data = peer.data.lock().unwrap();
        if data.waiting > 0 || data.revision != revision {
            return;
        }
        data.windows = snapshot.windows.clone();
        data.refresh_connection_failures(&snapshot.windows);
        data.connected = sessions
            .iter()
            .filter(|s| s.controlled && data.hosts.contains_key(&s.session_id))
            .map(|s| s.session_id.clone())
            .collect();
        let rules = service.rules_for(&endpoint.peer_id, &data, true);
        if rules.is_empty() {
            data.connection_failures.clear();
            if data.had_connection {
                data.planner.disarm();
            }
            return;
        }
        data.had_connection = true;
        data.planner.observe(&snapshot.windows, &rules, peer.now());
        if data.hosts.len() >= 16 {
            data.warning = "已达到接收窗口资源上限，待接入窗口会在关闭部分窗口后继续连接。".into();
            return;
        }
        if !snapshot.windows.is_empty()
            && snapshot.windows.iter().all(|w| w.application_key.is_none())
        {
            data.warning = "远端版本尚不支持新窗口自动接入，请更新发布端。".into();
            return;
        }
        data.warning.clear();
        let candidate = data.planner.candidate(
            &snapshot.windows,
            &rules,
            data.manual.values().cloned(),
            peer.now(),
        );
        candidate.map(|(window, request)| {
            // Follow an active application's selected frame-rate cap; do not
            // silently promote a 30-fps connection to 60 fps.
            let fps = service
                .takeover(&endpoint.peer_id)
                .map(|d| d.video_fps)
                .unwrap_or_else(|| data.hosts.values().map(|h| h.fps).min().unwrap_or(30));
            let background = data.planner.background(&window.id);
            (window, request, fps, background)
        })
    };
    let Some((window, request, fps, background)) = candidate else {
        return;
    };
    let key = window
        .application_key
        .clone()
        .or_else(|| window.app_id.clone())
        .unwrap_or_else(|| window.id.clone());
    // The operation gate covers selection, preparation and host registration.
    // A manual request arriving now reuses this exact session after the gate.
    let result = crate::launch_connected(
        app.clone(),
        endpoint.clone(),
        window.id.clone(),
        request,
        Some(1),
        None,
        window.icon_png,
        Some(window.name),
        Some(fps),
        Some(key),
        background,
    )
    .await;
    let mut data = peer.data.lock().unwrap();
    match result {
        Ok(_) => {
            data.planner.finish(&window.id, false, peer.now());
            data.connection_failures.remove(&window.id);
        }
        Err(error) => {
            data.planner
                .finish(&window.id, retry_failure(&error), peer.now());
            data.connection_failures
                .insert(window.id, format!("新窗口暂未接入：{}", error.message));
        }
    }
}

fn retry_failure(error: &crate::LaunchFailure) -> bool {
    // A parent scope can temporarily overlap a dialog acquired earlier in the
    // same inventory. Retry after the next inventory/lease release, without
    // stealing ownership or manufacturing a second launch request.
    error.retry_same_request
        || matches!(
            error.code.as_deref(),
            Some("session_limit" | "session_controlled")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn temporary_scope_and_resource_failures_retry_but_do_not_hide_terminal_failures() {
        for (code, retry) in [
            ("session_limit", true),
            ("session_controlled", true),
            ("window_unavailable", false),
            ("desktop_authorization_denied", false),
        ] {
            let error = crate::LaunchFailure {
                kind: "remote".into(),
                message: code.into(),
                retry_same_request: false,
                code: Some(code.into()),
            };
            assert_eq!(retry_failure(&error), retry, "{code}");
        }
    }
    #[test]
    fn successful_inventory_does_not_hide_a_failed_connection() {
        let service = Service::open(std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()));
        let p = peer();
        service
            .peers
            .lock()
            .unwrap()
            .insert("device".into(), p.clone());
        let mut window: DesktopWindowInfo = serde_json::from_value(serde_json::json!({
            "id":"chooser","window_id":1,"name":"Files","title":"Select",
            "icon_png":null,"minimized":false,"controlled":false
        }))
        .unwrap();
        {
            let mut data = p.data.lock().unwrap();
            data.connection_failures
                .insert("chooser".into(), "connection failed".into());
            data.warning = "inventory failed".into();
        }
        assert_eq!(service.status("device").warning, "inventory failed");
        {
            let mut data = p.data.lock().unwrap();
            data.warning.clear();
            data.refresh_connection_failures(&[window.clone()]);
        }
        assert_eq!(service.status("device").warning, "connection failed");
        window.controlled = true;
        p.data
            .lock()
            .unwrap()
            .refresh_connection_failures(&[window]);
        assert!(service.status("device").warning.is_empty());
        {
            let mut data = p.data.lock().unwrap();
            data.connection_failures
                .insert("closed".into(), "closed failure".into());
            data.refresh_connection_failures(&[]);
        }
        assert!(service.status("device").warning.is_empty());
    }
    fn peer() -> Arc<Peer> {
        Arc::new(Peer {
            gate: tokio::sync::Mutex::new(()),
            data: Mutex::new(Data::default()),
            clock: Instant::now(),
        })
    }
    #[test]
    fn host_reuse_is_session_scoped_and_closing_one_host_preserves_the_other() {
        let p = peer();
        let first = p.register(
            "one",
            Source {
                application_key: "gpt".into(),
                target_id: "window-one".into(),
            },
            11,
            30,
        );
        p.register(
            "two",
            Source {
                application_key: "idea".into(),
                target_id: "window-two".into(),
            },
            22,
            60,
        );
        assert_eq!(p.existing("window-one", None).unwrap().pid, 11);
        assert_eq!(p.existing("gpt", None).unwrap().pid, 11);
        assert_eq!(p.existing("", Some("two")).unwrap().pid, 22);
        p.closed("one", 999, first);
        assert!(p.existing("gpt", None).is_some());
        p.closed("one", 11, first);
        assert!(p.existing("gpt", None).is_none());
        assert_eq!(p.existing("idea", None).unwrap().pid, 22);
    }
    #[test]
    fn late_close_from_an_old_registration_cannot_remove_a_reopened_session_in_the_same_process() {
        let p = peer();
        let source = Source {
            application_key: "editor".into(),
            target_id: "window-one".into(),
        };
        let old = p.register("one", source.clone(), 42, 30);
        let current = p.register("one", source, 42, 60);
        p.closed("one", 42, old);
        assert_eq!(p.existing("window-one", None).unwrap().pid, 42);
        p.closed("one", 42, current);
        assert!(p.existing("window-one", None).is_none());
    }
    #[test]
    fn manual_intent_precedes_async_work_and_unknown_outcome_stays_reserved() {
        let p = peer();
        {
            let _intent = p.manual("request", "gpt");
            assert_eq!(p.data.lock().unwrap().waiting, 1);
        }
        assert_eq!(p.data.lock().unwrap().waiting, 0);
        assert_eq!(p.data.lock().unwrap().manual.get("request").unwrap(), "gpt");
        let intent = p.manual("request", "gpt");
        intent.resolve();
        drop(intent);
        assert!(p.data.lock().unwrap().manual.is_empty());
    }
    #[test]
    fn persisted_rules_are_per_application_and_device_and_do_not_enable_disconnected_hosts() {
        let dir = std::env::temp_dir().join(format!("auto-connect-{}", uuid::Uuid::new_v4()));
        let path = dir.join("rules.json");
        let service = Service::load(path.clone()).unwrap();
        let peer_id = uuid::Uuid::new_v4().to_string();
        for key in ["gpt", "idea"] {
            service
                .save(Preference {
                    peer_id: peer_id.clone(),
                    application_key: key.into(),
                    name: key.into(),
                    rule: Rule {
                        mode: Mode::All,
                        applications: vec![],
                    },
                })
                .unwrap();
        }
        let loaded = Service::load(path).unwrap();
        assert!(loaded.configured_for(&peer_id, "gpt"));
        assert!(loaded.configured_for(&peer_id, "idea"));
        assert!(!loaded.configured_for(&peer_id, "unconfigured"));
        assert!(!loaded.configured_for("other-device", "gpt"));
        let p = peer();
        assert!(loaded
            .active_rules(&peer_id, &p.data.lock().unwrap())
            .is_empty());
        p.register(
            "g",
            Source {
                application_key: "gpt".into(),
                target_id: "w-g".into(),
            },
            1,
            30,
        );
        p.register(
            "i",
            Source {
                application_key: "idea".into(),
                target_id: "w-i".into(),
            },
            2,
            30,
        );
        assert_eq!(
            loaded.active_rules(&peer_id, &p.data.lock().unwrap()).len(),
            2
        );
        assert!(loaded
            .active_rules("other-device", &p.data.lock().unwrap())
            .is_empty());
        p.data.lock().unwrap().connected.insert("i".into());
        assert_eq!(
            loaded
                .rules_for(&peer_id, &p.data.lock().unwrap(), true)
                .len(),
            1
        );
        let registration = p.data.lock().unwrap().hosts["g"].registration;
        p.closed("g", 1, registration);
        assert_eq!(
            loaded.active_rules(&peer_id, &p.data.lock().unwrap()).len(),
            1
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn damaged_settings_disable_only_automatic_connections_and_preserve_the_file() {
        let dir =
            std::env::temp_dir().join(format!("auto-connect-invalid-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path, b"broken settings").unwrap();
        let service = Service::open(path.clone());
        assert!(service.settings.lock().unwrap().rules.is_empty());
        assert!(!service.status("device").warning.is_empty());
        assert_eq!(fs::read(path).unwrap(), b"broken settings");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn device_mode_migrates_legacy_rules_and_is_independent_of_connected_applications() {
        let dir = std::env::temp_dir().join(format!("takeover-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let id = uuid::Uuid::new_v4().to_string();
        let legacy = vec![Preference {
            peer_id: id.clone(),
            application_key: "editor".into(),
            name: "Editor".into(),
            rule: Rule {
                mode: Mode::Selected,
                applications: vec!["chooser".into()],
            },
        }];
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let service = Service::load(path.clone()).unwrap();
        service
            .save_device(DevicePreference {
                peer_id: id.clone(),
                enabled: true,
                video_fps: 30,
            })
            .unwrap();
        let loaded = Service::load(path.clone()).unwrap();
        let data = Data::default();
        assert_eq!(loaded.rules_for(&id, &data, true)[0].mode, Mode::All);
        assert!(loaded
            .rules_for(&uuid::Uuid::new_v4().to_string(), &data, false)
            .is_empty());
        assert!(loaded.status(&id).takeover_enabled);
        assert_eq!(loaded.takeover(&id).unwrap().video_fps, 30);
        loaded
            .save_device(DevicePreference {
                peer_id: id.clone(),
                enabled: false,
                video_fps: 30,
            })
            .unwrap();
        assert!(!loaded.status(&id).takeover_enabled);
        assert_eq!(loaded.status(&id).rules[0].rule, legacy[0].rule);
        let before = fs::read(&path).unwrap();
        assert!(loaded
            .save_device(DevicePreference {
                peer_id: id.clone(),
                enabled: true,
                video_fps: 90
            })
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disconnect_all_targets_only_this_peers_owned_hosts_and_stops_automatic_following() {
        let p = peer();
        let other = peer();
        let (stop, stopped) = std::sync::mpsc::channel();
        let (other_stop, other_stopped) = std::sync::mpsc::channel();
        for (peer, sender) in [(&p, stop), (&other, other_stop)] {
            let registration = peer.register(
                "s",
                Source {
                    application_key: "editor".into(),
                    target_id: "window-editor".into(),
                },
                42,
                30,
            );
            peer.set_stop("s", registration, sender);
        }
        p.disconnect_all();
        stopped.try_recv().unwrap();
        assert!(other_stopped.try_recv().is_err());
        assert!(p.data.lock().unwrap().disconnecting);
        assert!(!other.data.lock().unwrap().disconnecting);
        assert_eq!(
            p.data.lock().unwrap().hosts.len(),
            1,
            "retain ownership until the process exits"
        );
    }
    #[tokio::test]
    async fn manual_open_waiting_for_automatic_preparation_reuses_the_registered_receiver() {
        let p = peer();
        let automatic = p.gate.lock().await;
        let intent = p.manual("manual-request", "window-chooser");
        let manual_peer = p.clone();
        let manual = tokio::spawn(async move {
            let _gate = manual_peer.gate.lock().await;
            let result = manual_peer.existing("window-chooser", None).unwrap();
            intent.resolve();
            result.pid
        });
        p.register(
            "chooser-session",
            Source {
                application_key: "chooser".into(),
                target_id: "window-chooser".into(),
            },
            42,
            30,
        );
        drop(automatic);
        assert_eq!(manual.await.unwrap(), 42);
        assert_eq!(p.data.lock().unwrap().hosts.len(), 1);
        assert_eq!(p.data.lock().unwrap().waiting, 0);
    }
}
