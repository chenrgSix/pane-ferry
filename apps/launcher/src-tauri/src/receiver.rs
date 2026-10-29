//! Windows window lifecycle and device-bound RWP stream. Webviews never choose
//! a route or session. Pulling one message at a time bounds the UI video backlog.
use crate::{
    auto_connect, local_rpc,
    network::{self, Device},
};
use paneferry_platform::ipc::LocalStream;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::{
    io::{BufReader, ReadHalf, WriteHalf},
    sync::{watch, Mutex as AsyncMutex},
};

struct Connection {
    reader: AsyncMutex<BufReader<ReadHalf<LocalStream>>>,
    writer: AsyncMutex<WriteHalf<LocalStream>>,
    windows: Mutex<HashSet<u64>>,
    stop: watch::Sender<bool>,
}
struct Entry {
    group: String,
    target: Option<String>,
    session: String,
    route: PathBuf,
    fps: u8,
    epoch: u64,
    connection: Option<Arc<Connection>>,
    on_close: Option<Box<dyn FnOnce() + Send>>,
}
fn entries() -> &'static Mutex<HashMap<String, Entry>> {
    static ENTRIES: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(Default::default)
}
pub fn lookup(
    group: &str,
    session: Option<&str>,
    target: Option<&str>,
) -> Result<Option<String>, String> {
    Ok(entries()
        .lock()
        .unwrap()
        .values()
        .find(|entry| {
            entry.group == group
                && (session == Some(entry.session.as_str())
                    || target.is_some() && target == entry.target.as_deref())
        })
        .map(|entry| entry.session.clone()))
}
pub fn spawn(
    app: &tauri::AppHandle,
    group: String,
    device: &Device,
    session: &str,
    fps: u8,
    source: Option<auto_connect::Source>,
    background: bool,
) -> Result<u32, String> {
    let existing = entries()
        .lock()
        .unwrap()
        .iter()
        .find(|(_, e)| e.group == group && e.session == session)
        .map(|(label, _)| label.clone());
    if let Some(window) = existing.and_then(|label| app.get_webview_window(&label)) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(std::process::id());
    }
    let route = network::target(app, device)
        .map_err(|e| e.to_string())?
        .socket;
    let label = format!("receiver-{}", uuid::Uuid::new_v4());
    let target = source.as_ref().map(|s| s.target_id.clone());
    entries().lock().unwrap().insert(
        label.clone(),
        Entry {
            group,
            target,
            session: session.into(),
            route,
            fps,
            epoch: 0,
            connection: None,
            on_close: None,
        },
    );
    let window =
        match WebviewWindowBuilder::new(app, &label, WebviewUrl::App("receiver.html".into()))
            .title("远程应用 — 窗渡 PaneFerry")
            .inner_size(1000.0, 720.0)
            .min_inner_size(400.0, 300.0)
            .visible(false)
            .focused(!background)
            .build()
        {
            Ok(window) => window,
            Err(_) => {
                remove(&label);
                return Err("接收窗口创建失败，远端会话已保留。".into());
            }
        };
    // The publisher must never recursively publish a received remote window.
    let marked = window.hwnd().is_ok_and(|hwnd| unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetPropW(
            hwnd.0 as _,
            windows_sys::w!("PaneFerry.RemoteWindow.1"),
            1usize as _,
        ) != 0
    });
    if !marked {
        let _ = window.destroy();
        remove(&label);
        return Err("无法标记接收窗口，远端会话已保留。".into());
    }
    let _ = window.show();
    if !background {
        let _ = window.set_focus();
    }
    if let Some(source) = source {
        let peer = app.state::<auto_connect::Service>().peer(app, device);
        let pid = std::process::id();
        let registration = peer.register(session, source, pid, fps);
        let (stop, requests) = std::sync::mpsc::channel();
        peer.set_stop(session, registration, stop);
        let session = session.to_owned();
        let close = move || peer.closed(&session, pid, registration);
        if let Some(entry) = entries().lock().unwrap().get_mut(&label) {
            entry.on_close = Some(Box::new(close));
        }
        std::thread::spawn(move || {
            if requests.recv().is_ok() {
                let _ = window.close();
            }
        });
    }
    Ok(std::process::id())
}
pub fn disconnect(label: &str) {
    if let Some(entry) = entries().lock().unwrap().get_mut(label) {
        entry.epoch += 1;
        if let Some(connection) = entry.connection.take() {
            connection.stop.send_replace(true);
        }
    }
}
pub fn remove(label: &str) {
    let entry = entries().lock().unwrap().remove(label);
    if let Some(mut entry) = entry {
        if let Some(connection) = entry.connection.take() {
            connection.stop.send_replace(true);
        }
        if let Some(close) = entry.on_close.take() {
            close();
        }
    }
}
fn connection(label: &str, epoch: u64) -> Result<(Arc<Connection>, String), String> {
    let entries = entries().lock().unwrap();
    let entry = entries
        .get(label)
        .filter(|e| e.epoch == epoch)
        .ok_or("连接已失效。")?;
    Ok((
        entry.connection.clone().ok_or("连接尚未就绪。")?,
        entry.session.clone(),
    ))
}
fn check(value: &Value) -> Result<(), String> {
    if value["version"] != 1 {
        return Err("远端协议不兼容。".into());
    }
    if value["type"] == "error" {
        let code = value["code"].as_str().unwrap_or("protocol");
        let safe: String = code
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(64)
            .collect();
        return Err(format!("remote:{safe}"));
    }
    Ok(())
}
pub async fn open(label: &str, selected_fps: Option<u8>) -> Result<Value, String> {
    if let Some(fps) = selected_fps {
        crate::requested_video_fps(Some(fps)).map_err(|e| e.to_string())?;
        if let Some(entry) = entries().lock().unwrap().get_mut(label) {
            entry.fps = fps;
        }
    }
    disconnect(label);
    let (route, session, fps, epoch) = {
        let entries = entries().lock().unwrap();
        let entry = entries.get(label).ok_or("窗口已关闭。")?;
        (
            entry.route.clone(),
            entry.session.clone(),
            entry.fps,
            entry.epoch,
        )
    };
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let stream = LocalStream::connect(route).await.map_err(|_| "设备连接尚未恢复。")?;
        let (input, mut output) = stream.into_split();
        let mut input = BufReader::new(input);
        local_rpc::write(&mut output, &json!({"type":"hello","version":1})).await?;
        let hello = local_rpc::read(&mut input).await?;
        check(&hello)?;
        let capabilities = hello["capabilities"].as_array().ok_or("远端能力无效。")?;
        if hello["type"] != "hello" || !capabilities.iter().any(|c| c == "native.h264") {
            return Err("远端不支持 H.264 接收。".into());
        }
        let qos = capabilities.iter().any(|c| c == "video.qos");
        let max_fps = (fps as u64).min(hello["video_max_fps"].as_u64().filter(|n| (1..=90).contains(n)).unwrap_or(90));
        let mut enable = json!({"type":"video.enable","version":1,"codec":"h264"});
        if qos || capabilities.iter().any(|c| c == "platform.windows") {
            enable["max_fps"] = json!(max_fps);
        }
        // No feedback until a frame is actually painted by the receiver.
        if qos { enable["feedback"] = json!(true); }
        local_rpc::write(&mut output, &enable).await?;
        let ready = local_rpc::read(&mut input).await?;
        check(&ready)?;
        if ready["type"] != "video.ready" { return Err("视频协商失败。".into()); }
        local_rpc::write(&mut output, &json!({"type":"session.resume","version":1,"session_id":session})).await?;
        let snapshot = local_rpc::read(&mut input).await?;
        check(&snapshot)?;
        if snapshot["type"] != "session.snapshot" || snapshot["session_id"] != session {
            return Err("会话响应无效。".into());
        }
        let windows = snapshot["windows"].as_array().ok_or("窗口目录无效。")?;
        if windows.is_empty() || windows.len() > 128 { return Err("窗口目录无效。".into()); }
        let ids: HashSet<u64> = windows.iter().filter_map(|w| w["window_id"].as_u64().filter(|id| *id > 0)).collect();
        if ids.len() != windows.len() { return Err("窗口目录无效。".into()); }
        let (stop, _) = watch::channel(false);
        let connection = Arc::new(Connection { reader: AsyncMutex::new(input), writer: AsyncMutex::new(output), windows: Mutex::new(ids), stop });
        let mut entries = entries().lock().unwrap();
        let entry = entries.get_mut(label).filter(|e| e.epoch == epoch).ok_or("连接已失效。")?;
        entry.connection = Some(connection);
        Ok(json!({"epoch":epoch,"snapshot":snapshot,"capabilities":capabilities,"max_fps":max_fps,"selected_fps":fps}))
    }).await.map_err(|_| "连接超时，请稍后重试。".to_string())?;
    result
}
pub async fn next(label: &str, epoch: u64) -> Result<Value, String> {
    let (connection, session) = connection(label, epoch)?;
    let mut stopped = connection.stop.subscribe();
    if *stopped.borrow() {
        return Err("连接已关闭。".into());
    }
    let value = tokio::select! {
        _ = stopped.changed() => return Err("连接已关闭。".into()),
        value = async {
            let mut reader = connection.reader.lock().await;
            local_rpc::read(&mut *reader).await
        } => value?,
    };
    if value["version"] != 1
        || (value["session_id"] != session
            && !(value["type"] == "error" && value["session_id"] == ""))
    {
        return Err("远端会话身份不匹配。".into());
    }
    if value["type"] == "window.created" {
        if let Some(id) = value["window_id"].as_u64().filter(|id| *id > 0) {
            let mut windows = connection.windows.lock().unwrap();
            if windows.len() >= 128 {
                return Err("窗口数量超限。".into());
            }
            windows.insert(id);
        }
    } else if value["type"] == "window.destroyed" {
        if let Some(id) = value["window_id"].as_u64() {
            connection.windows.lock().unwrap().remove(&id);
        }
    }
    Ok(value)
}
pub async fn send(label: &str, epoch: u64, mut body: Value) -> Result<(), String> {
    let (connection, _) = connection(label, epoch)?;
    if *connection.stop.borrow() {
        return Err("连接已关闭。".into());
    }
    let kind = body["type"].as_str().ok_or("请求无效。")?;
    if !matches!(
        kind,
        "input.pointer"
            | "input.key"
            | "input.key.logical"
            | "input.text.commit"
            | "window.focus"
            | "window.configure"
            | "video.keyframe"
            | "video.feedback"
    ) {
        return Err("接收窗口请求无效。".into());
    }
    if kind != "video.feedback"
        && !body["window_id"]
            .as_u64()
            .is_some_and(|id| connection.windows.lock().unwrap().contains(&id))
    {
        return Err("窗口已失效。".into());
    }
    if body.to_string().len() > 64 * 1024 {
        return Err("输入过长。".into());
    }
    if kind == "input.text.commit" {
        paneferry_protocol::validate_text(body["text"].as_str().ok_or("文本无效。")?)
            .map_err(|_| "文本无效。")?;
    } else if kind == "window.configure" {
        let width = body["width"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("尺寸无效。")?;
        let height = body["height"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("尺寸无效。")?;
        paneferry_protocol::dimensions(width, height).map_err(|_| "尺寸无效。")?;
    }
    body["version"] = json!(1);
    let mut writer = connection.writer.lock().await;
    local_rpc::write(&mut *writer, &body).await
}

#[cfg(test)]
#[path = "windows/receiver_tests.rs"]
mod tests;
