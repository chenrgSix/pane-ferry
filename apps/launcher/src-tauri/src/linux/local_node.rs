use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
};
use tauri::Manager;

static START: Mutex<()> = Mutex::new(());
fn socket(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .home_dir()
        .map_err(|_| "无法定位本机服务。")?
        .join(".local/state/lan-remoteapp/agent.sock"))
}
fn exchange(path: &Path, mut body: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(path).map_err(|_| "本机服务尚未就绪。")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "本机服务连接失败。")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "本机服务连接失败。")?;
    stream
        .write_all(b"{\"type\":\"hello\",\"version\":1}\n")
        .map_err(|_| "本机服务连接失败。")?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|_| "本机服务连接失败。")?);
    fn read(reader: &mut BufReader<UnixStream>) -> Result<Value, String> {
        let mut line = String::new();
        reader
            .take(16 * 1024 + 1)
            .read_line(&mut line)
            .map_err(|_| "本机服务响应失败。")?;
        if line.len() > 16 * 1024 || !line.ends_with('\n') {
            return Err("本机服务响应无效。".into());
        }
        serde_json::from_str(&line).map_err(|_| "本机服务响应无效。".into())
    }
    if read(&mut reader)?["type"] != "hello" {
        return Err("本机服务协议不兼容。".into());
    }
    body["version"] = json!(1);
    let mut data = serde_json::to_vec(&body).map_err(|_| "请求无效。")?;
    data.push(b'\n');
    stream.write_all(&data).map_err(|_| "本机服务连接失败。")?;
    let result = read(&mut reader)?;
    if result["type"] == "error" {
        return Err("本机共享操作失败，请检查桌面和服务状态。".into());
    }
    Ok(result)
}
pub fn ensure(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let _guard = START.lock().map_err(|_| "本机服务启动失败。")?;
    let socket = socket(app)?;
    let probe = || exchange(&socket, json!({"type":"node.info"}));
    if probe().is_ok() {
        return Ok(socket);
    }
    let installation = crate::linux::runtime::installation(app)?;
    // Start is idempotent and never restarts the existing agent or its apps.
    let started = Command::new("systemctl")
        .args(["--user", "start", "remoteapp-agent.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !started {
        let mut child = Command::new(installation.agent)
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "本机 Agent 启动失败，请检查安装。")?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    for _ in 0..60 {
        if probe().is_ok() {
            return Ok(socket);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("本机 Agent 启动超时，请检查用户服务。".into())
}
pub fn publisher(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    ensure(app)
}
pub fn info(app: &tauri::AppHandle) -> Result<Value, String> {
    let mut value = exchange(&ensure(app)?, json!({"type":"node.info"}))?;
    value["platform"] = json!("linux");
    if let Ok(directory) = app.path().app_data_dir() {
        let path = directory.join("launcher-defaults.json");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= 1024) {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(defaults) = serde_json::from_slice::<Value>(&bytes) {
                    value["launcher_defaults"] = defaults;
                }
            }
        }
    }
    Ok(value)
}
pub fn sharing(app: &tauri::AppHandle, enabled: bool) -> Result<Value, String> {
    exchange(
        &ensure(app)?,
        json!({"type":"sharing.set", "enabled":enabled}),
    )?;
    info(app)
}
pub fn permissions(app: &tauri::AppHandle) -> Result<Value, String> {
    info(app)
}
pub fn reveal_component(app: &tauri::AppHandle) -> Result<(), String> {
    let installation = crate::linux::runtime::installation(app)?;
    Command::new("xdg-open")
        .arg(installation.receiver.parent().ok_or("安装目录无效。")?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "无法打开本机组件目录。")?;
    Ok(())
}
