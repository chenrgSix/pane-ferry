use remoteapp_transport::{self as transport, Endpoint};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::Manager;

#[derive(Default)]
pub struct LocalNode {
    routes: Mutex<HashMap<String, Arc<Route>>>,
}
struct Route {
    stop: AtomicBool,
    status: Mutex<String>,
}
fn directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位本机服务。")?
        .join("Node"))
}
fn component(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("RemoteAppNode.app", tauri::path::BaseDirectory::Resource)
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
    let mut value = exchange(&ensure(app)?, json!({"type":"node.info"}))?;
    let state = app.state::<LocalNode>();
    let routes = state.routes.lock().unwrap();
    value["routes"] = json!(routes
        .iter()
        .map(|(key, route)| json!({"endpoint":key,"state":*route.status.lock().unwrap()}))
        .collect::<Vec<_>>());
    Ok(value)
}
pub fn sharing(app: &tauri::AppHandle, enabled: bool) -> Result<Value, String> {
    let result = exchange(
        &ensure(app)?,
        json!({"type":"sharing.set","enabled":enabled}),
    )?;
    if !enabled {
        for route in app
            .state::<LocalNode>()
            .routes
            .lock()
            .unwrap()
            .drain()
            .map(|(_, value)| value)
        {
            route.stop.store(true, Ordering::SeqCst);
        }
    };
    Ok(result)
}
pub fn permissions(app: &tauri::AppHandle) -> Result<Value, String> {
    exchange(&ensure(app)?, json!({"type":"permissions.request"}))
}
pub fn offer(app: &tauri::AppHandle, endpoint: &Endpoint, helper: &Path) -> Result<(), String> {
    endpoint.validate().map_err(|error| error.to_string())?;
    let local = exchange(&ensure(app)?, json!({"type":"node.info"}))?;
    if local["sharing_enabled"] != true {
        return Ok(());
    }
    let id = local["device_id"]
        .as_str()
        .ok_or("本机设备身份无效。")?
        .to_owned();
    let name = local["name"].as_str().unwrap_or("Mac").to_owned();
    let key = serde_json::to_string(endpoint).map_err(|_| "连接无效。")?;
    let state = app.state::<LocalNode>();
    let mut routes = state.routes.lock().unwrap();
    if routes.contains_key(&key) {
        return Ok(());
    }
    let route = Arc::new(Route {
        stop: AtomicBool::new(false),
        status: Mutex::new("连接中".into()),
    });
    routes.insert(key, route.clone());
    let endpoint = endpoint.clone();
    let helper = helper.to_owned();
    let socket = directory(app)?.join("node.sock");
    tauri::async_runtime::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        let mut attempt = 0;
        while !route.stop.load(Ordering::SeqCst) {
            let result: Result<(), String> = async {
                let prepared = transport::prepare_peer(&endpoint, &id)
                    .await
                    .map_err(|error| error.to_string())?;
                let remote = prepared["socket"].as_str().ok_or("返回通道响应无效。")?;
                let generation = prepared["generation"]
                    .as_str()
                    .ok_or("返回通道响应无效。")?;
                if generation.len() != 32 || !generation.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err("返回通道响应无效。".into());
                }
                let password = Command::new(&helper)
                    .args([
                        "--credential-check",
                        &endpoint.host,
                        &endpoint.user,
                        &endpoint.port.to_string(),
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                let args = transport::reverse_args(
                    &endpoint,
                    password,
                    remote,
                    socket.to_str().ok_or("返回通道地址无效。")?,
                )
                .map_err(|error| error.to_string())?;
                let mut command = tokio::process::Command::new("/usr/bin/ssh");
                command
                    .args(args)
                    .env("LC_ALL", "C")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                if password {
                    command
                        .env("SSH_ASKPASS", &helper)
                        .env("SSH_ASKPASS_REQUIRE", "force")
                        .env(
                            "REMOTEAPP_SSH_ASKPASS_ACCOUNT",
                            format!("{}@{}:{}", endpoint.user, endpoint.host, endpoint.port),
                        );
                }
                let mut child = command.spawn().map_err(|_| "返回通道启动失败。")?;
                let mut input = child.stdin.take().ok_or("返回通道启动失败。")?;
                input
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"device_id":id,"name":name,"generation":generation})
                        )
                        .as_bytes(),
                    )
                    .await
                    .map_err(|_| "返回通道连接失败。")?;
                let mut output =
                    tokio::io::BufReader::new(child.stdout.take().ok_or("返回通道启动失败。")?)
                        .take(32);
                let mut ready = String::new();
                tokio::time::timeout(Duration::from_secs(15), output.read_line(&mut ready))
                    .await
                    .map_err(|_| "返回通道连接超时。")?
                    .map_err(|_| "返回通道连接失败。")?;
                if ready.trim() != "ready" {
                    return Err("返回通道尚未就绪，请检查 Ubuntu 是否已升级。".into());
                }
                *route.status.lock().unwrap() = "已连接".into();
                attempt = 0;
                let mut ticks = 0;
                loop {
                    if route.stop.load(Ordering::SeqCst) {
                        let _ = child.kill().await;
                        return Ok(());
                    }
                    if child.try_wait().map_err(|_| "返回通道断开。")?.is_some() {
                        return Err("连接暂时中断，正在恢复".into());
                    }
                    ticks += 1;
                    if ticks % 20 == 0 {
                        input
                            .write_all(b".\n")
                            .await
                            .map_err(|_| "返回通道断开。")?;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            .await;
            if let Err(message) = result {
                *route.status.lock().unwrap() = message;
            }
            let delay = [2, 4, 8, 15][attempt.min(3)];
            attempt += 1;
            for _ in 0..delay * 2 {
                if route.stop.load(Ordering::SeqCst) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    });
    Ok(())
}
