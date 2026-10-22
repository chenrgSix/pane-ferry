use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use tauri::Manager;

fn directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位本机服务。")?
        .join("Node"))
}
fn component(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("PaneFerryNode.app", tauri::path::BaseDirectory::Resource)
        .map_err(|_| "缺少本机共享组件。".into())
}
pub fn reveal_component(app: &tauri::AppHandle) -> Result<(), String> {
    // Reveal the exact bundled copy used by ensure, never a development copy
    // found by display name or LaunchServices' duplicate bundle registration.
    let status = Command::new("/usr/bin/open")
        .arg("-R")
        .arg(component(app)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "未能显示本机共享组件。")?;
    if status.success() {
        Ok(())
    } else {
        Err("未能显示本机共享组件。".into())
    }
}
fn exchange(path: &Path, body: Value) -> Result<Value, String> {
    let mut socket = UnixStream::connect(path).map_err(|_| "本机服务尚未就绪。")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "本机服务连接失败。")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "本机服务连接失败。")?;
    socket
        .write_all(b"{\"type\":\"hello\",\"version\":1}\n")
        .map_err(|_| "本机服务连接失败。")?;
    let mut reader = BufReader::new(socket.try_clone().map_err(|_| "本机服务连接失败。")?);
    let mut line = String::new();
    reader
        .by_ref()
        .take(16384)
        .read_line(&mut line)
        .map_err(|_| "本机服务响应失败。")?;
    let mut body = body;
    body["version"] = json!(1);
    let mut data = serde_json::to_vec(&body).map_err(|_| "请求无效。")?;
    data.push(b'\n');
    socket.write_all(&data).map_err(|_| "本机服务连接失败。")?;
    line.clear();
    reader
        .take(16384)
        .read_line(&mut line)
        .map_err(|_| "本机服务响应失败。")?;
    let result: Value = serde_json::from_str(&line).map_err(|_| "本机服务响应无效。")?;
    if result["type"] == "error" {
        return Err("本机共享操作失败，请检查系统权限。".into());
    };
    Ok(result)
}
pub fn ensure(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let socket = directory(app)?.join("management.sock");
    if UnixStream::connect(&socket).is_ok() {
        return Ok(socket);
    }
    // LaunchServices gives the accessory app its own TCC responsibility.
    // Directly spawning its executable attributes permission checks to this
    // Rust launcher, so a user's grant to the helper cannot take effect.
    let status = Command::new("/usr/bin/open")
        .args(["-g", "-n", "-a"])
        .arg(component(app)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "本机服务启动失败。")?;
    if !status.success() {
        return Err("本机服务启动失败。".into());
    }
    for _ in 0..50 {
        if UnixStream::connect(&socket).is_ok() {
            return Ok(socket);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("本机服务启动超时。".into())
}
pub fn info(app: &tauri::AppHandle) -> Result<Value, String> {
    exchange(&ensure(app)?, json!({"type":"node.info"}))
}
pub fn sharing(app: &tauri::AppHandle, enabled: bool) -> Result<Value, String> {
    exchange(
        &ensure(app)?,
        json!({"type":"sharing.set","enabled":enabled}),
    )
}
pub fn permissions(app: &tauri::AppHandle) -> Result<Value, String> {
    exchange(&ensure(app)?, json!({"type":"permissions.request"}))
}
