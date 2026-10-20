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
    mode: String,
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
async fn launch_app(
    app: tauri::AppHandle,
    endpoint: Endpoint,
    app_id: String,
    mode: String,
    xpra_path: String,
    request_id: String,
) -> Result<LaunchResult, String> {
    endpoint.validate().map_err(|e| e.to_string())?;
    transport::validate_id(&app_id).map_err(|e| e.to_string())?;
    transport::validate_id(&request_id).map_err(|e| e.to_string())?;
    match mode.as_str() {
        "xpra" => {
            let path = PathBuf::from(xpra_path);
            if !path.is_absolute() || !path.is_file() {
                return Err("找不到 Xpra，请填写已安装客户端的绝对可执行路径。".into());
            }
            let session = transport::launch_xpra(&endpoint, &app_id, &request_id)
                .await
                .map_err(|e| e.to_string())?;
            let pid = transport::spawn_xpra(&endpoint, &session.display, &path).map_err(|e| {
                format!(
                    "{} 远端会话 {} 已创建，显示号 {}；请不要重复创建。",
                    e, session.session_id, session.display
                )
            })?;
            Ok(LaunchResult {
                mode,
                session_id: Some(session.session_id),
                display: Some(session.display),
                pid,
                message: "Xpra 客户端已启动，请以实际窗口确认连接结果。断开不等于结束远端应用。"
                    .into(),
            })
        }
        "native" => {
            if !cfg!(target_os = "macos") {
                return Err("原生 WindowHost 目前只支持 macOS。".into());
            }
            let path = app
                .path()
                .resolve("RemoteAppWindowHost", tauri::path::BaseDirectory::Resource)
                .map_err(|_| "无法定位 WindowHost。")?;
            if !path.is_file() {
                return Err("请先运行 scripts/build-macos.sh 打包 WindowHost。".into());
            }
            let session = transport::launch_native(&endpoint, &app_id, &request_id)
                .await
                .map_err(|e| e.to_string())?;
            let mut child = Command::new(path)
                .args([
                    "--host",
                    &endpoint.host,
                    "--user",
                    &endpoint.user,
                    "--port",
                    &endpoint.port.to_string(),
                    "--session",
                    &session.session_id,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| {
                    format!(
                        "原生宿主启动失败。远端会话 {} 已创建，重试会复用该会话。",
                        session.session_id
                    )
                })?;
            let pid = child.id();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(LaunchResult {
                mode,
                session_id: Some(session.session_id),
                display: Some(session.display),
                pid,
                message: "原生宿主已启动，连接结果由宿主显示。尚未通过真实设备联调。".into(),
            })
        }
        _ => Err("未知后端。".into()),
    }
}
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![list_apps, launch_app])
        .run(tauri::generate_context!())
        .expect("launcher startup failed");
}
