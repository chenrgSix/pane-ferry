#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use remoteapp_transport::{self as transport, AppCatalog, Endpoint};
use serde::Serialize;
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
use tauri::Manager;
mod credential_store;
mod local_node;
mod ssh_login;
mod window_host;
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
async fn share_to_device(app: tauri::AppHandle, endpoint: Endpoint) -> Result<(), String> {
    local_node::offer(&app, &endpoint, &native_host(&app)?)
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
}
impl From<transport::Error> for LaunchFailure {
    fn from(error: transport::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
            retry_same_request: error.retry_same_request(),
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
async fn list_apps(endpoint: Endpoint) -> Result<AppCatalog, RequestFailure> {
    transport::catalog(&endpoint)
        .await
        .map_err(RequestFailure::from)
}
#[tauri::command]
async fn list_desktop_windows(
    endpoint: Endpoint,
) -> Result<transport::DesktopWindows, RequestFailure> {
    transport::desktop_windows(&endpoint)
        .await
        .map_err(RequestFailure::from)
}
#[tauri::command]
async fn list_sessions(endpoint: Endpoint) -> Result<Vec<transport::SessionInfo>, RequestFailure> {
    transport::sessions(&endpoint)
        .await
        .map_err(RequestFailure::from)
}
#[tauri::command]
async fn open_ssh_login(app: tauri::AppHandle, endpoint: Endpoint) -> Result<(), String> {
    endpoint.validate().map_err(|error| error.to_string())?;
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|_| "无法定位本机登录目录。")?;
    ssh_login::open(&cache, &endpoint)
}
#[tauri::command]
async fn save_ssh_password(
    app: tauri::AppHandle,
    endpoint: Endpoint,
    password: String,
) -> Result<(), String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位连接配置目录。")?;
    credential_store::save(&directory, endpoint, password)
}
fn native_host(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("原生 WindowHost 目前只支持 macOS。".into());
    }
    let path = app
        .path()
        .resolve(
            "RemoteAppWindowHost.app/Contents/MacOS/RemoteAppWindowHost",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|_| "无法定位 WindowHost。")?;
    if !path.is_file() {
        return Err("请先运行 scripts/build-macos.sh 打包 WindowHost。".into());
    }
    Ok(path)
}
fn named_native_host(
    app: &tauri::AppHandle,
    endpoint: &Endpoint,
    app_id: &str,
    app_name: Option<&str>,
) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位本机应用目录。")?
        .join("WindowHosts");
    window_host::prepare(&directory, &native_host(app)?, endpoint, app_id, app_name)
}
fn spawn_host(
    path: PathBuf,
    endpoint: &Endpoint,
    session_id: &str,
    icon_png: Option<String>,
) -> Result<u32, String> {
    // Icons are already in the launcher catalog/window cache. Deliver once over
    // stdin, without command-line image data, temporary files or another scan.
    let icon = icon_png.filter(|value| !value.is_empty() && value.len() <= 24 * 1024);
    let mut command = Command::new(path);
    command.args([
        "--host",
        &endpoint.host,
        "--user",
        &endpoint.user,
        "--port",
        &endpoint.port.to_string(),
        "--session",
        session_id,
    ]);
    if icon.is_some() {
        command.arg("--icon-stdin");
    }
    let mut child = command
        .stdin(if icon.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("原生宿主启动失败。远端会话 {session_id} 已保留，可重新连接。"))?;
    let pid = child.id();
    std::thread::spawn(move || {
        // A missing or corrupt icon never prevents the session from connecting.
        if let (Some(mut input), Some(icon)) = (child.stdin.take(), icon) {
            let _ = input.write_all(icon.as_bytes());
        }
        let _ = child.wait();
    });
    Ok(pid)
}
#[tauri::command]
async fn resume_session(
    app: tauri::AppHandle,
    endpoint: Endpoint,
    session_id: String,
    icon_png: Option<String>,
    app_name: Option<String>,
) -> Result<LaunchResult, RequestFailure> {
    endpoint.validate()?;
    transport::validate_id(&session_id)?;
    let session = transport::sessions(&endpoint)
        .await
        .map_err(RequestFailure::from)?
        .into_iter()
        .find(|s| s.session_id == session_id)
        .ok_or_else(|| RequestFailure::local("远端会话已退出，请刷新列表。"))?;
    if session.controlled {
        return Err(RequestFailure::local(
            "该会话已连接，请先在原宿主中断开，再重新连接。",
        ));
    }
    let pid = spawn_host(
        named_native_host(&app, &endpoint, &session.app_id, app_name.as_deref())
            .map_err(RequestFailure::local)?,
        &endpoint,
        &session_id,
        icon_png,
    )
    .map_err(RequestFailure::local)?;
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
    endpoint: Endpoint,
    app_id: String,
    request_id: String,
    render_scale: Option<u8>,
    icon_png: Option<String>,
    app_name: Option<String>,
) -> Result<LaunchResult, LaunchFailure> {
    endpoint.validate()?;
    transport::validate_id(&app_id)?;
    transport::validate_id(&request_id)?;
    let path =
        named_native_host(&app, &endpoint, &app_id, app_name.as_deref()).map_err(|message| {
            LaunchFailure {
                kind: "client",
                message,
                retry_same_request: false,
            }
        })?;
    let scale = render_scale.unwrap_or_else(|| {
        if app
            .get_webview_window("main")
            .and_then(|window| window.scale_factor().ok())
            .unwrap_or(1.0)
            >= 1.5
        {
            2
        } else {
            1
        }
    });
    let session = transport::launch_native(&endpoint, &app_id, &request_id, scale).await?;
    let pid = spawn_host(path, &endpoint, &session.session_id, icon_png).map_err(|message| {
        LaunchFailure {
            kind: "client",
            message,
            retry_same_request: true,
        }
    })?;
    Ok(LaunchResult {
        session_id: Some(session.session_id),
        display: Some(session.display),
        pid,
        message: if app_id.starts_with("window-") {
            "现有窗口客户端已启动，原应用的内容和登录状态保留。".into()
        } else {
            format!(
                "原生窗口已启动；分辨率随本机窗口调整，界面缩放 {}%。",
                session.scale_factor * 100
            )
        },
    })
}
fn main() {
    tauri::Builder::default()
        .manage(local_node::LocalNode::default())
        .setup(|app| {
            if let Ok(path) = native_host(app.handle()) {
                transport::set_password_helper(path);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_apps,
            list_desktop_windows,
            list_sessions,
            launch_app,
            resume_session,
            open_ssh_login,
            save_ssh_password,
            local_node_info,
            set_local_sharing,
            request_local_permissions,
            share_to_device
        ])
        .build(tauri::generate_context!())
        .expect("launcher startup failed")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        });
}
