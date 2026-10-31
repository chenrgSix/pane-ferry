//! One player per paired device, owned by the launcher rather than WindowHosts.
use crate::network::{self, Device};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::Manager;
use tokio::sync::Notify;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Status {
    enabled: bool,
    state: String,
    message: String,
}
#[derive(Default)]
struct Inner {
    enabled: BTreeSet<String>,
    statuses: BTreeMap<String, Status>,
    tasks: BTreeMap<String, tauri::async_runtime::JoinHandle<()>>,
    generations: BTreeMap<String, u64>,
}
pub struct Service {
    path: PathBuf,
    inner: Arc<Mutex<Inner>>,
    wake: Arc<Notify>,
}
impl Service {
    pub fn open(path: PathBuf) -> Self {
        let enabled: BTreeSet<String> = std::fs::read(&path)
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        let enabled = enabled
            .into_iter()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok_and(|u| u.to_string() == *id))
            .collect();
        Self {
            path,
            inner: Arc::new(Mutex::new(Inner {
                enabled,
                ..Default::default()
            })),
            wake: Arc::new(Notify::new()),
        }
    }
    fn set(&self, id: String, enabled: bool) -> Result<Status, String> {
        let mut inner = self.inner.lock().unwrap();
        let mut settings = inner.enabled.clone();
        if enabled {
            settings.insert(id.clone());
        } else {
            settings.remove(&id);
        }
        let save = || -> Result<(), Box<dyn std::error::Error>> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let pending = self.path.with_extension("tmp");
            std::fs::write(&pending, serde_json::to_vec(&settings)?)?;
            std::fs::rename(pending, &self.path)?;
            Ok(())
        };
        save().map_err(|_| "声音设置保存失败，请重试。".to_string())?;
        inner.enabled = settings;
        *inner.generations.entry(id.clone()).or_default() += 1;
        // Cancelling drops the IPC subscription and the playback stop guard.
        if let Some(task) = inner.tasks.remove(&id) {
            task.abort();
        }
        let status = Status {
            enabled,
            state: if enabled { "connecting" } else { "off" }.into(),
            message: String::new(),
        };
        inner.statuses.insert(id, status.clone());
        self.wake.notify_one();
        Ok(status)
    }
    pub fn stop(&self) {
        for (_, task) in std::mem::take(&mut self.inner.lock().unwrap().tasks) {
            task.abort();
        }
    }
    pub fn forget(&self, peer: &str) {
        let _ = self.set(peer.into(), false);
    }
    pub fn start(&self, app: &tauri::AppHandle) {
        let app = app.clone();
        let inner = self.inner.clone();
        let wake = self.wake.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let ids: Vec<_> = inner.lock().unwrap().enabled.iter().cloned().collect();
                for id in ids {
                    let mut state = inner.lock().unwrap();
                    if !state.enabled.contains(&id) {
                        continue;
                    }
                    // Tasks remove themselves only through the manager below;
                    // completion is recorded in state, avoiding duplicate players.
                    if state.tasks.contains_key(&id)
                        && state
                            .statuses
                            .get(&id)
                            .is_some_and(|s| s.state != "unavailable" && s.state != "unsupported")
                    {
                        continue;
                    }
                    if let Some(task) = state.tasks.remove(&id) {
                        task.abort();
                    }
                    state.statuses.insert(
                        id.clone(),
                        Status {
                            enabled: true,
                            state: "connecting".into(),
                            message: String::new(),
                        },
                    );
                    let generation = *state.generations.entry(id.clone()).or_default();
                    let app = app.clone();
                    let shared = inner.clone();
                    let peer = id.clone();
                    let task = tauri::async_runtime::spawn(async move {
                        let result = receive(&app, &peer, &shared, generation).await;
                        let mut state = shared.lock().unwrap();
                        if state.enabled.contains(&peer)
                            && state.generations.get(&peer) == Some(&generation)
                        {
                            let message = result
                                .err()
                                .unwrap_or_else(|| "声音连接已结束，将自动重试。".into());
                            let unsupported =
                                message == "发布端尚不支持系统声音，请更新 Windows 发布端。";
                            state.statuses.insert(
                                peer,
                                Status {
                                    enabled: true,
                                    state: if unsupported {
                                        "unsupported"
                                    } else {
                                        "unavailable"
                                    }
                                    .into(),
                                    message,
                                },
                            );
                        }
                    });
                    state.tasks.insert(id, task);
                }
                tokio::select! { _ = wake.notified() => {}, _ = tokio::time::sleep(Duration::from_secs(3)) => {} }
            }
        });
    }
}
#[tauri::command]
pub fn device_audio_status(app: tauri::AppHandle) -> BTreeMap<String, Status> {
    let service = app.state::<Service>();
    let inner = service.inner.lock().unwrap();
    let mut statuses = inner.statuses.clone();
    for id in &inner.enabled {
        statuses.entry(id.clone()).or_insert_with(|| Status {
            enabled: true,
            state: "connecting".into(),
            message: String::new(),
        });
    }
    statuses
}
#[tauri::command]
pub async fn set_device_audio(
    app: tauri::AppHandle,
    endpoint: Device,
    enabled: bool,
) -> Result<Status, String> {
    endpoint
        .validate()
        .map_err(|_| "设备身份无效。".to_string())?;
    if enabled {
        let app = app.clone();
        let endpoint = endpoint.clone();
        tauri::async_runtime::spawn_blocking(move || network::target(&app, &endpoint))
            .await
            .map_err(|_| "声音组件启动失败。".to_string())?
            .map_err(|_| "请先配对这台设备。".to_string())?;
    }
    app.state::<Service>().set(endpoint.peer_id, enabled)
}

#[cfg(not(any(windows, target_os = "macos")))]
async fn receive(
    _: &tauri::AppHandle,
    _: &str,
    _: &Arc<Mutex<Inner>>,
    _: u64,
) -> Result<(), String> {
    Err("本机暂不支持接收系统声音。".into())
}

#[cfg(any(windows, target_os = "macos"))]
async fn receive(
    app: &tauri::AppHandle,
    peer: &str,
    shared: &Arc<Mutex<Inner>>,
    generation: u64,
) -> Result<(), String> {
    use base64::Engine;
    use serde_json::{json, Value};
    use std::time::Instant;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    async fn read<R: tokio::io::AsyncBufRead + Unpin>(read: &mut R) -> Result<Value, String> {
        let mut data = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let bytes = read.fill_buf().await.map_err(|_| "声音连接已断开。")?;
                if bytes.is_empty() {
                    return Err("声音连接已断开。");
                }
                let count = bytes
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(bytes.len(), |i| i + 1);
                if data.len() + count > 8192 {
                    return Err("声音数据无效。");
                }
                data.extend_from_slice(&bytes[..count]);
                read.consume(count);
                if data.last() == Some(&b'\n') {
                    break;
                }
            }
            serde_json::from_slice(&data).map_err(|_| "声音数据无效。")
        })
        .await
        .map_err(|_| "声音连接超时，将自动重试。".to_string())?
        .map_err(str::to_string)
    }
    async fn send<W: tokio::io::AsyncWrite + Unpin>(
        write: &mut W,
        value: Value,
    ) -> Result<(), String> {
        let mut data = serde_json::to_vec(&value).unwrap();
        data.push(b'\n');
        tokio::time::timeout(Duration::from_millis(500), write.write_all(&data))
            .await
            .map_err(|_| "声音连接超时。".to_string())?
            .map_err(|_| "声音连接已断开。".to_string())
    }
    let app = app.clone();
    let id = peer.to_string();
    let target = tauri::async_runtime::spawn_blocking(move || {
        network::target(&app, &Device { peer_id: id })
    })
    .await
    .map_err(|_| "声音连接失败。")?
    .map_err(|_| "设备离线或配对已撤销。")?;
    let socket = tokio::time::timeout(
        Duration::from_secs(5),
        paneferry_platform::ipc::LocalStream::connect(&target.socket),
    )
    .await
    .map_err(|_| "声音连接超时。")?
    .map_err(|_| "设备声音连接失败，将自动重试。")?;
    let (read_half, mut write) = socket.into_split();
    let mut reader = BufReader::new(read_half);
    send(&mut write, json!({"type":"hello","version":1})).await?;
    let hello = read(&mut reader).await?;
    if hello["type"] != "hello" {
        return Err("设备声音连接失败，将自动重试。".into());
    }
    if !hello["capabilities"]
        .as_array()
        .is_some_and(|v| v.iter().any(|c| c == paneferry_audio::CAPABILITY))
    {
        return Err("发布端尚不支持系统声音，请更新 Windows 发布端。".into());
    }
    let player = tauri::async_runtime::spawn_blocking(paneferry_audio::Player::start)
        .await
        .map_err(|_| "声音播放器启动失败。")?
        .map_err(|_| "本机播放设备不可用，请检查音频输出。")?;
    send(&mut write, json!({"type":"audio.subscribe","version":1})).await?;
    let format = read(&mut reader).await?;
    if format["type"] != "audio.format"
        || format["codec"] != "opus"
        || format["sample_rate"] != 48000
        || format["channels"] != 2
        || format["frame_samples"] != 480
    {
        return Err("远端系统声音暂不可用，请检查 Windows 默认播放设备和共享开关。".into());
    }
    {
        let mut inner = shared.lock().unwrap();
        if !inner.enabled.contains(peer) || inner.generations.get(peer) != Some(&generation) {
            return Ok(());
        }
        inner.statuses.insert(
            peer.into(),
            Status {
                enabled: true,
                state: "playing".into(),
                message: String::new(),
            },
        );
    }
    // Compare source elapsed time to local elapsed time, without assuming clocks
    // are synchronized. A minimum arrival offset detects queued reliable packets.
    let clock = Instant::now();
    let mut timing = Timing::default();
    let mut last = 0;
    loop {
        let value = read(&mut reader).await?;
        if !player.healthy() {
            return Err("本机播放设备已变化，正在重新连接。".into());
        }
        match value["type"].as_str() {
            Some("audio.heartbeat") => {}
            Some("audio.frame") => {
                let sequence = value["sequence"].as_u64().ok_or("声音数据无效。")?;
                let stamp = value["timestamp_ms"]
                    .as_u64()
                    .filter(|v| *v < i64::MAX as u64)
                    .ok_or("声音数据无效。")?;
                if sequence <= last {
                    return Err("声音序列无效。".into());
                }
                last = sequence;
                let data = value["data"]
                    .as_str()
                    .filter(|s| s.len() <= 1700)
                    .ok_or("声音数据无效。")?;
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| "声音数据无效。")?;
                if timing.fresh(clock.elapsed().as_millis() as i64, stamp as i64) {
                    player
                        .push(paneferry_audio::Packet {
                            sequence,
                            created: Instant::now(),
                            data,
                        })
                        .map_err(|_| "声音播放暂时中断，正在恢复。")?;
                }
                send(
                    &mut write,
                    json!({"version":1,"type":"audio.ack","sequence":sequence}),
                )
                .await?;
            }
            Some("error") => return Err("远端声音暂时中断，正在恢复。".into()),
            _ => return Err("声音数据无效。".into()),
        }
    }
}

#[derive(Default)]
struct Timing {
    offset: Option<i64>,
    adjusted_at: i64,
}
impl Timing {
    fn fresh(&mut self, arrival: i64, source: i64) -> bool {
        let delta = arrival - source;
        let baseline = self.offset.get_or_insert(delta);
        // Accommodate independent hardware/monotonic clock drift on long sessions
        // without treating a burst of queued packets as a new latency baseline.
        let drift = arrival.saturating_sub(self.adjusted_at) / 1000;
        if drift > 0 {
            *baseline = baseline.saturating_add(drift);
            self.adjusted_at = arrival;
        }
        *baseline = (*baseline).min(delta);
        delta.saturating_sub(*baseline) <= 80
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_off_and_per_device_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.json");
        let service = Service::open(path.clone());
        assert!(service.inner.lock().unwrap().enabled.is_empty());
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        service.set(first.clone(), true).unwrap();
        service.set(second.clone(), true).unwrap();
        service.set(first.clone(), false).unwrap();
        let loaded = Service::open(path);
        assert_eq!(
            loaded.inner.lock().unwrap().enabled,
            BTreeSet::from([second])
        );
        assert_eq!(service.inner.lock().unwrap().statuses[&first].state, "off");
    }
    #[test]
    fn delayed_audio_is_dropped_without_requiring_synchronized_clocks() {
        let mut timing = Timing::default();
        assert!(timing.fresh(100, 5000));
        assert!(timing.fresh(115, 5010));
        assert!(!timing.fresh(400, 5020));
        assert!(timing.fresh(410, 5310));
    }
}
