#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use paneferry_transport::{self as transport, AppCatalog};
use serde::Serialize;
#[cfg(not(windows))]
use std::{path::PathBuf, process::Command};
use tauri::Manager;
mod legacy_settings;
#[cfg_attr(windows, path = "windows/local_node.rs")]
mod local_node;
#[cfg_attr(windows, path = "windows/network.rs")]
mod network;
use network::Device;
mod auto_connect;
#[cfg(not(windows))]
mod host_group;
#[cfg(windows)]
mod receiver;
#[cfg(not(windows))]
mod window_host;
#[cfg(windows)]
use receiver as host_group;
#[cfg(windows)]
#[path = "windows/rpc.rs"]
mod local_rpc;
#[tauri::command]
async fn local_node_info(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    local_node::info(&app)
}
#[tauri::command]
async fn set_local_sharing(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    local_node::sharing(&app, enabled)
}
#[tauri::command]
async fn request_local_permissions(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    local_node::permissions(&app)
}
#[tauri::command]
async fn reveal_local_component(app: tauri::AppHandle) -> Result<(), String> {
    local_node::reveal_component(&app)
}
#[tauri::command]
async fn network_action(
    app: tauri::AppHandle,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match body["op"].as_str() {
        Some("status" | "invite" | "pair" | "revoke" | "address") => network::manage(&app, body),
        _ => Err("操作无效。".into()),
    }
}
#[derive(Serialize)]
struct LaunchResult {
    session_id: Option<String>,
    display: Option<String>,
    pid: u32,
    message: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchFailure {
    kind: &'static str,
    message: String,
    retry_same_request: bool,
    code: Option<String>,
}
impl From<transport::Error> for LaunchFailure {
    fn from(error: transport::Error) -> Self {
        let code = match &error {
            transport::Error::Remote(code) => Some(code.clone()),
            _ => None,
        };
        Self {
            kind: error.kind(),
            message: error.to_string(),
            retry_same_request: error.retry_same_request(),
            code,
        }
    }
}
#[derive(Serialize)]
struct RequestFailure {
    kind: &'static str,
    message: String,
}
impl From<transport::Error> for RequestFailure {
    fn from(error: transport::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}
impl RequestFailure {
    fn local(message: impl Into<String>) -> Self {
        Self {
            kind: "client",
            message: message.into(),
        }
    }
}
#[tauri::command]
async fn list_apps(app: tauri::AppHandle, endpoint: Device) -> Result<AppCatalog, RequestFailure> {
    transport::catalog(&network::target(&app, &endpoint)?)
        .await
        .map_err(RequestFailure::from)
}
#[tauri::command]
async fn list_desktop_windows(
    app: tauri::AppHandle,
    endpoint: Device,
) -> Result<transport::DesktopWindows, RequestFailure> {
    transport::desktop_windows(&network::target(&app, &endpoint)?)
        .await
        .map_err(RequestFailure::from)
}
#[tauri::command]
async fn list_desktop_monitors(
    app: tauri::AppHandle,
    endpoint: Device,
) -> Result<transport::DesktopMonitors, RequestFailure> {
    transport::desktop_monitors(&network::target(&app, &endpoint)?)
        .await
        .map_err(RequestFailure::from)
}
#[tauri::command]
async fn list_sessions(
    app: tauri::AppHandle,
    endpoint: Device,
) -> Result<Vec<transport::SessionInfo>, RequestFailure> {
    transport::sessions(&network::target(&app, &endpoint)?)
        .await
        .map_err(RequestFailure::from)
}
#[cfg(not(windows))]
fn native_host(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("原生 WindowHost 目前只支持 macOS。".into());
    }
    let path = app
        .path()
        .resolve(
            "PaneFerryWindowHost.app/Contents/MacOS/PaneFerryWindowHost",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|_| "无法定位 WindowHost。")?;
    if !path.is_file() {
        return Err("请先运行 scripts/build-macos.sh 打包 WindowHost。".into());
    }
    Ok(path)
}
struct NativeHost {
    #[cfg(not(windows))]
    executable: PathBuf,
    group: String,
}
#[cfg(windows)]
fn named_native_host(
    _app: &tauri::AppHandle,
    endpoint: &Device,
    app_id: &str,
    _name: Option<&str>,
) -> Result<NativeHost, String> {
    use sha2::{Digest, Sha256};
    let identity =
        serde_json::to_vec(&(1, endpoint.identity(), app_id)).map_err(|_| "应用身份无效。")?;
    Ok(NativeHost {
        group: format!("{:x}", Sha256::digest(identity)),
    })
}
#[cfg(not(windows))]
fn named_native_host(
    app: &tauri::AppHandle,
    endpoint: &Device,
    app_id: &str,
    app_name: Option<&str>,
) -> Result<NativeHost, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位本机应用目录。")?
        .join("WindowHosts");
    let executable = window_host::prepare(
        &directory,
        &native_host(app)?,
        &endpoint.identity(),
        app_id,
        app_name,
    )?;
    Ok(NativeHost {
        executable,
        group: window_host::application_identity(&endpoint.identity(), app_id)
            .map_err(|_| "无法准备应用入口。")?,
    })
}
#[cfg(not(windows))]
fn spawn_host(
    app: &tauri::AppHandle,
    path: NativeHost,
    endpoint: &Device,
    session_id: &str,
    icon_png: Option<String>,
    video_fps: u8,
    source: Option<auto_connect::Source>,
    background: bool,
) -> Result<u32, String> {
    let peer = app.state::<auto_connect::Service>().peer(app, endpoint);
    if let Some(existing) = peer.existing("", Some(session_id)) {
        return Ok(existing.pid);
    }
    // Icons are already in the launcher catalog/window cache. Deliver once over
    // stdin, without command-line image data, temporary files or another scan.
    let icon = icon_png.filter(|value| !value.is_empty() && value.len() <= 24 * 1024);
    let mut command = Command::new(path.executable);
    command
        .arg("--native-peer")
        .arg(&endpoint.peer_id)
        .arg("--native-state")
        .arg(network::directory(app)?)
        .arg("--native-helper")
        .arg(network::binary(app)?)
        .arg("--session")
        .arg(session_id)
        .arg("--fps")
        .arg(video_fps.to_string());
    if background {
        command.arg("--background-start");
    }
    let attached = host_group::attach_or_start(
        &path.group,
        &mut command,
        session_id,
        source.as_ref().map(|s| s.target_id.as_str()),
        video_fps,
        background,
        icon.as_deref(),
    )?;
    let pid = attached.pid;
    let (stop, stop_requests) = std::sync::mpsc::channel();
    let registration = source.map(|source| {
        let registration = peer.register(session_id, source, pid, video_fps);
        peer.set_stop(session_id, registration, stop);
        registration
    });
    let session_id = session_id.to_owned();
    attached.watch(stop_requests, move || {
        if let Some(registration) = registration {
            peer.closed(&session_id, pid, registration);
        }
    });
    Ok(pid)
}
#[cfg(windows)]
fn spawn_host(
    app: &tauri::AppHandle,
    path: NativeHost,
    endpoint: &Device,
    session_id: &str,
    _icon: Option<String>,
    video_fps: u8,
    source: Option<auto_connect::Source>,
    background: bool,
) -> Result<u32, String> {
    receiver::spawn(
        app, path.group, endpoint, session_id, video_fps, source, background,
    )
}
fn requested_video_fps(value: Option<u8>) -> Result<u8, transport::Error> {
    match value.unwrap_or(60) {
        fps @ (30 | 60) => Ok(fps),
        _ => Err(transport::Error::Invalid("请选择 30 或 60 帧/秒。")),
    }
}
#[cfg(test)]
mod frame_rate_tests {
    use super::*;
    #[test]
    fn default_and_explicit_choices_are_bounded_before_any_remote_operation() {
        assert_eq!(requested_video_fps(None).unwrap(), 60);
        for fps in [30, 60] {
            assert_eq!(requested_video_fps(Some(fps)).unwrap(), fps);
        }
        for fps in [0, 1, 24, 31, 59, 61, 90, 255] {
            assert!(matches!(
                requested_video_fps(Some(fps)),
                Err(transport::Error::Invalid(_))
            ));
        }
    }
}
#[tauri::command]
async fn resume_session(
    app: tauri::AppHandle,
    endpoint: Device,
    session_id: String,
    icon_png: Option<String>,
    app_name: Option<String>,
    video_fps: Option<u8>,
    application_key: Option<String>,
) -> Result<LaunchResult, RequestFailure> {
    endpoint.validate()?;
    let video_fps = requested_video_fps(video_fps)?;
    transport::validate_id(&session_id)?;
    if let Some(key) = &application_key {
        transport::validate_id(key)?;
    }
    let peer = app.state::<auto_connect::Service>().peer(&app, &endpoint);
    let intent = peer.manual(&format!("resume-{session_id}"), &session_id);
    let _gate = peer.gate.lock().await;
    if let Some(result) = peer.existing("", Some(&session_id)) {
        intent.resolve();
        return Ok(result);
    }
    let session = transport::sessions(&network::target(&app, &endpoint)?)
        .await
        .map_err(RequestFailure::from)?
        .into_iter()
        .find(|s| s.session_id == session_id)
        .ok_or_else(|| RequestFailure::local("远端会话已退出，请刷新列表。"))?;
    let key = application_key.as_deref().unwrap_or(&session.app_id);
    let path = named_native_host(&app, &endpoint, key, app_name.as_deref())
        .map_err(RequestFailure::local)?;
    if session.controlled
        && host_group::lookup(&path.group, Some(&session_id), None)
            .map_err(RequestFailure::local)?
            .is_none()
    {
        return Err(RequestFailure::local(
            "该会话已连接，请先在原宿主中断开，再重新连接。",
        ));
    }
    let source = (session.mode.as_deref() != Some("desktop")).then(|| auto_connect::Source {
        application_key: application_key.unwrap_or_else(|| session.app_id.clone()),
        target_id: session.app_id.clone(),
    });
    if source.as_ref().is_some_and(|source| {
        app.state::<auto_connect::Service>()
            .configured_for(&endpoint.peer_id, &source.application_key)
    }) {
        app.state::<auto_connect::Service>()
            .prepare_baseline(&app, &endpoint, &peer)
            .await;
    }
    let pid = spawn_host(
        &app,
        path,
        &endpoint,
        &session_id,
        icon_png,
        video_fps,
        source,
        false,
    )
    .map_err(RequestFailure::local)?;
    intent.resolve();
    Ok(LaunchResult {
        session_id: Some(session_id),
        display: Some(session.display.to_string()),
        pid,
        message: "原生窗口客户端已启动，请以实际窗口确认连接结果。".into(),
    })
}
#[tauri::command]
async fn launch_app(
    app: tauri::AppHandle,
    endpoint: Device,
    app_id: String,
    request_id: String,
    render_scale: Option<u8>,
    scene: Option<transport::SceneSize>,
    icon_png: Option<String>,
    app_name: Option<String>,
    video_fps: Option<u8>,
    application_key: Option<String>,
) -> Result<LaunchResult, LaunchFailure> {
    endpoint.validate()?;
    requested_video_fps(video_fps)?;
    transport::validate_id(&app_id)?;
    transport::validate_id(&request_id)?;
    if let Some(key) = &application_key {
        transport::validate_id(key)?;
    }
    let peer = app.state::<auto_connect::Service>().peer(&app, &endpoint);
    let intent = peer.manual(&request_id, &app_id);
    let _gate = peer.gate.lock().await;
    if let Some(result) = peer.existing(&app_id, None) {
        intent.resolve();
        return Ok(result);
    }
    if app.state::<auto_connect::Service>().configured_for(
        &endpoint.peer_id,
        application_key.as_deref().unwrap_or(&app_id),
    ) {
        app.state::<auto_connect::Service>()
            .prepare_baseline(&app, &endpoint, &peer)
            .await;
    }
    let result = launch_connected(
        app,
        endpoint,
        app_id,
        request_id,
        render_scale,
        scene,
        icon_png,
        app_name,
        video_fps,
        application_key,
        false,
    )
    .await;
    if result
        .as_ref()
        .err()
        .is_none_or(|error| !error.retry_same_request)
    {
        intent.resolve();
    }
    result
}
async fn launch_connected(
    app: tauri::AppHandle,
    endpoint: Device,
    app_id: String,
    request_id: String,
    render_scale: Option<u8>,
    scene: Option<transport::SceneSize>,
    icon_png: Option<String>,
    app_name: Option<String>,
    video_fps: Option<u8>,
    application_key: Option<String>,
    background: bool,
) -> Result<LaunchResult, LaunchFailure> {
    endpoint.validate()?;
    let video_fps = requested_video_fps(video_fps)?;
    transport::validate_id(&app_id)?;
    transport::validate_id(&request_id)?;
    let path = named_native_host(
        &app,
        &endpoint,
        application_key.as_deref().unwrap_or(&app_id),
        app_name.as_deref(),
    )
    .map_err(|message| LaunchFailure {
        kind: "client",
        message,
        retry_same_request: false,
        code: None,
    })?;
    if let Some(session_id) =
        host_group::lookup(&path.group, None, Some(&app_id)).map_err(|message| LaunchFailure {
            kind: "client",
            message,
            retry_same_request: true,
            code: None,
        })?
    {
        let pid = spawn_host(
            &app,
            path,
            &endpoint,
            &session_id,
            icon_png,
            video_fps,
            Some(auto_connect::Source {
                application_key: application_key.unwrap_or_else(|| app_id.clone()),
                target_id: app_id,
            }),
            background,
        )
        .map_err(|message| LaunchFailure {
            kind: "client",
            message,
            retry_same_request: true,
            code: None,
        })?;
        return Ok(LaunchResult {
            session_id: Some(session_id),
            display: None,
            pid,
            message: "该窗口已连接，继续使用现有接收窗口。".into(),
        });
    }
    // Startup uses ordinary resolution. Existing sessions retain source scale.
    let scale = render_scale.unwrap_or(1);
    let session = transport::launch_native(
        &network::target(&app, &endpoint)?,
        &app_id,
        &request_id,
        scale,
        scene,
    )
    .await?;
    let pid = spawn_host(
        &app,
        path,
        &endpoint,
        &session.session_id,
        icon_png,
        video_fps,
        Some(auto_connect::Source {
            application_key: application_key.unwrap_or_else(|| app_id.clone()),
            target_id: app_id.clone(),
        }),
        background,
    )
    .map_err(|message| LaunchFailure {
        kind: "client",
        message,
        retry_same_request: true,
        code: None,
    })?;
    Ok(LaunchResult {
        session_id: Some(session.session_id),
        display: Some(session.display),
        pid,
        message: if app_id.starts_with("window-") {
            "现有窗口客户端已启动，原应用的内容和登录状态保留。".into()
        } else {
            "已连接应用窗口；已有窗口保留原缩放。实际接收像素与放大比例见窗口标题。".into()
        },
    })
}
#[tauri::command]
async fn start_desktop(
    app: tauri::AppHandle,
    endpoint: Device,
    monitor_id: String,
    monitor_name: String,
    request_id: String,
    video_fps: Option<u8>,
) -> Result<LaunchResult, LaunchFailure> {
    endpoint.validate()?;
    let video_fps = requested_video_fps(video_fps)?;
    transport::validate_id(&monitor_id)?;
    transport::validate_id(&request_id)?;
    let name = format!(
        "Ubuntu 桌面 · {}",
        monitor_name.chars().take(80).collect::<String>()
    );
    let path = named_native_host(
        &app,
        &endpoint,
        &format!("screen-{monitor_id}"),
        Some(&name),
    )
    .map_err(|message| LaunchFailure {
        kind: "client",
        message,
        retry_same_request: false,
        code: None,
    })?;
    let session =
        transport::start_desktop(&network::target(&app, &endpoint)?, &monitor_id, &request_id)
            .await?;
    let pid = spawn_host(
        &app,
        path,
        &endpoint,
        &session.session_id,
        None,
        video_fps,
        None,
        false,
    )
    .map_err(|message| LaunchFailure {
        kind: "client",
        message,
        retry_same_request: true,
        code: None,
    })?;
    Ok(LaunchResult {
        session_id: Some(session.session_id),
        display: Some(session.display),
        pid,
        message: "已连接当前登录的桌面；关闭本机窗口只会断开连接。".into(),
    })
}
fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let settings = app.path().app_data_dir()?.join("automatic-windows.json");
            app.manage(auto_connect::Service::open(settings));
            app.state::<auto_connect::Service>().start(app.handle());
            if let Ok(directory) = app.path().app_data_dir() {
                if legacy_settings::retire(&directory).is_err() {
                    eprintln!("旧连接配置清理失败；当前仅支持设备配对连接。");
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(windows)]
            if window.label().starts_with("receiver-") {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    receiver::remove(window.label());
                }
                return;
            }
            #[cfg(not(windows))]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_apps,
            list_desktop_windows,
            list_desktop_monitors,
            list_sessions,
            launch_app,
            start_desktop,
            resume_session,
            auto_connect::automatic_window_status,
            auto_connect::set_automatic_window_rule,
            auto_connect::set_device_window_takeover,
            auto_connect::disconnect_device_windows,
            local_node_info,
            set_local_sharing,
            request_local_permissions,
            reveal_local_component,
            network_action,
            receiver_open,
            receiver_next,
            receiver_send,
            receiver_disconnect
        ])
        .build(tauri::generate_context!())
        .expect("launcher startup failed")
        .run(|app, event| {
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        });
}

// Keep command registration identical across platforms; receiver windows are
// created only on Windows. Other platforms retain their native hosts.
#[tauri::command]
async fn receiver_open(
    window: tauri::WebviewWindow,
    fps: Option<u8>,
) -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    {
        return receiver::open(window.label(), fps).await;
    }
    #[cfg(not(windows))]
    {
        let _ = (window, fps);
        Err("仅支持 Windows。".into())
    }
}
#[tauri::command]
async fn receiver_next(
    window: tauri::WebviewWindow,
    epoch: u64,
) -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    {
        return receiver::next(window.label(), epoch).await;
    }
    #[cfg(not(windows))]
    {
        let _ = (window, epoch);
        Err("仅支持 Windows。".into())
    }
}
#[tauri::command]
async fn receiver_send(
    window: tauri::WebviewWindow,
    epoch: u64,
    body: serde_json::Value,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        return receiver::send(window.label(), epoch, body).await;
    }
    #[cfg(not(windows))]
    {
        let _ = (window, epoch, body);
        Err("仅支持 Windows。".into())
    }
}
#[tauri::command]
async fn receiver_disconnect(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(windows)]
    receiver::disconnect(window.label());
    let _ = window;
    Ok(())
}
