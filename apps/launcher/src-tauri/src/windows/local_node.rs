use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
};
use tauri::Manager;

static START: Mutex<()> = Mutex::new(());
pub fn root() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("无法定位本机数据目录。")?)
            .join("PaneFerry"),
    )
}
fn component(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve(
            "paneferry-windows.exe",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|_| "缺少 Windows 共享组件。".into())
}
pub fn ensure(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let _guard = START.lock().map_err(|_| "本机服务启动失败。")?;
    let socket = root()?.join("node/management.sock");
    let probe =
        || crate::local_rpc::exchange(&socket, json!({"type":"node.info","version":1}), true);
    if probe().is_ok() {
        return Ok(socket);
    }
    let mut child = Command::new(component(app)?)
        .args(["serve", "--state"])
        .arg(root()?)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW; sharing starts disabled.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "共享组件启动失败，请重新构建 Windows 应用。")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    for _ in 0..60 {
        if probe().is_ok() {
            return Ok(socket);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("本机服务启动超时，请检查 UDP 42424 端口。".into())
}
pub fn info(app: &tauri::AppHandle) -> Result<Value, String> {
    crate::local_rpc::exchange(&ensure(app)?, json!({"type":"node.info","version":1}), true)
}
pub fn sharing(app: &tauri::AppHandle, enabled: bool) -> Result<Value, String> {
    crate::local_rpc::exchange(
        &ensure(app)?,
        json!({"type":"sharing.set","version":1,"enabled":enabled}),
        true,
    )?;
    info(app)
}
pub fn permissions(app: &tauri::AppHandle) -> Result<Value, String> {
    info(app)
}
pub fn reveal_component(app: &tauri::AppHandle) -> Result<(), String> {
    Command::new("explorer.exe")
        .arg(format!("/select,{}", component(app)?.display()))
        .spawn()
        .map_err(|_| "无法显示共享组件。")?;
    Ok(())
}
