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
    fps: u8,
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
    pub fn register(&self, session: &str, source: Source, pid: u32, fps: u8) {
        let mut data = self.data.lock().unwrap();
        data.revision += 1;
        data.hosts.insert(session.into(), Host { source, pid, fps });
    }
    pub fn closed(&self, session: &str, pid: u32) {
        let mut data = self.data.lock().unwrap();
        if !data.hosts.get(session).is_some_and(|h| h.pid == pid) {
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
    preferences: Mutex<Vec<Preference>>,
    peers: Mutex<HashMap<String, Arc<Peer>>>,
    settings_warning: Mutex<String>,
}
impl Service {
    pub fn configured_for(&self, peer_id: &str, application_key: &str) -> bool {
        self.preferences.lock().unwrap().iter().any(|p| {
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
            preferences: Mutex::new(vec![]),
            peers: Mutex::new(HashMap::new()),
            settings_warning: Mutex::new(format!("{error} 自动接入已停用，请重新保存设置。")),
        })
    }
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let rules = if path.exists() {
            if path.is_symlink()
                || fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024
            {
                return Err("自动接入设置文件无效。".into());
            }
            let rules: Vec<Preference> =
                serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|_| "自动接入设置无法读取。")?;
            if rules.len() > 256 {
                return Err("自动接入设置数量超限。".into());
            }
            for rule in &rules {
                rule.validate()?;
            }
            rules
        } else {
            vec![]
        };
        Ok(Self {
            path,
            preferences: Mutex::new(rules),
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
        self.preferences
            .lock()
            .unwrap()
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
        let rules = self
            .preferences
            .lock()
            .unwrap()
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
        let mut current = self.preferences.lock().unwrap();
        let mut next = current.clone();
        next.retain(|p| {
            p.peer_id != preference.peer_id || p.application_key != preference.application_key
        });
        next.push(preference);
        if next.len() > 256 {
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
            data.warning = "已连接的窗口较多，关闭部分窗口后继续自动接入。".into();
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
            let fps = data.hosts.values().map(|h| h.fps).min().unwrap_or(30);
            (window, request, fps)
        })
    };
    let Some((window, request, fps)) = candidate else {
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
                .finish(&window.id, error.retry_same_request, peer.now());
            data.connection_failures
                .insert(window.id, format!("新窗口暂未接入：{}", error.message));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        p.register(
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
        p.closed("one", 999);
        assert!(p.existing("gpt", None).is_some());
        p.closed("one", 11);
        assert!(p.existing("gpt", None).is_none());
        assert_eq!(p.existing("idea", None).unwrap().pid, 22);
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
        p.closed("g", 1);
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
        assert!(service.preferences.lock().unwrap().is_empty());
        assert!(!service.status("device").warning.is_empty());
        assert_eq!(fs::read(path).unwrap(), b"broken settings");
        fs::remove_dir_all(dir).unwrap();
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
