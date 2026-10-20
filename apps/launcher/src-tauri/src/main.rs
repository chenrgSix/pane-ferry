#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use remoteapp_transport::{self as transport, AppInfo, Endpoint};
use serde::Serialize;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
use tauri::Manager;
#[derive(Serialize)]
struct LaunchResult {
    session_id: Option<String>,
    display: Option<String>,
    pid: u32,
    message: String,
}
#[tauri::command]
async fn list_apps(endpoint: Endpoint) -> Result<Vec<AppInfo>, String> {
    transport::catalog(&endpoint)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn list_sessions(endpoint: Endpoint) -> Result<Vec<transport::SessionInfo>, String> {
    transport::sessions(&endpoint)
        .await
        .map_err(|e| e.to_string())
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
fn spawn_host(path: PathBuf, endpoint: &Endpoint, session_id: &str) -> Result<u32, String> {
    let mut child = Command::new(path)
        .args([
            "--host",
            &endpoint.host,
            "--user",
            &endpoint.user,
            "--port",
            &endpoint.port.to_string(),
            "--session",
            session_id,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("原生宿主启动失败。远端会话 {session_id} 已保留，可重新连接。"))?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}
#[tauri::command]
async fn resume_session(
    app: tauri::AppHandle,
    endpoint: Endpoint,
    session_id: String,
) -> Result<LaunchResult, String> {
    endpoint.validate().map_err(|e| e.to_string())?;
    transport::validate_id(&session_id).map_err(|e| e.to_string())?;
    let session = transport::sessions(&endpoint)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|s| s.session_id == session_id)
        .ok_or("远端会话已退出，请刷新列表。")?;
    if session.controlled {
        return Err("该会话已连接，请先在原宿主中断开，再重新连接。".into());
    }
    let pid = spawn_host(native_host(&app)?, &endpoint, &session_id)?;
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
) -> Result<LaunchResult, String> {
    endpoint.validate().map_err(|e| e.to_string())?;
    transport::validate_id(&app_id).map_err(|e| e.to_string())?;
    transport::validate_id(&request_id).map_err(|e| e.to_string())?;
    let path = native_host(&app)?;
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
    let session = transport::launch_native(&endpoint, &app_id, &request_id, scale)
        .await
        .map_err(|e| e.to_string())?;
    let pid = spawn_host(path, &endpoint, &session.session_id)?;
    Ok(LaunchResult {
        session_id: Some(session.session_id),
        display: Some(session.display),
        pid,
        message: format!(
            "原生窗口已启动；分辨率随本机窗口调整，界面缩放 {}%。",
            session.scale_factor * 100
        ),
    })
}
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_apps,
            list_sessions,
            launch_app,
            resume_session
        ])
        .run(tauri::generate_context!())
        .expect("launcher startup failed");
}
