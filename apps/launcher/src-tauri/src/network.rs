//! Only private, local IPC reaches device pairing and service management.
use remoteapp_transport::{Endpoint, Target};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
};
use tauri::Manager;
static START: Mutex<()> = Mutex::new(());
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub host: String,
    pub user: String,
    pub port: u16,
    #[serde(rename = "peerId", default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
}
impl Device {
    pub fn ssh(&self) -> Endpoint {
        Endpoint {
            host: self.host.clone(),
            user: self.user.clone(),
            port: self.port,
        }
    }
    pub fn validate(&self) -> Result<(), remoteapp_transport::Error> {
        if let Some(id) = &self.peer_id {
            if uuid::Uuid::parse_str(id).is_ok_and(|v| v.to_string() == *id) {
                Ok(())
            } else {
                Err(remoteapp_transport::Error::Invalid("设备身份无效。"))
            }
        } else {
            self.ssh().validate()
        }
    }
    pub fn identity(&self) -> Value {
        self.peer_id.as_ref().map_or_else(
            || json!(self.ssh()),
            |id| json!({"transport":"quic","device_id":id}),
        )
    }
}
pub fn directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .home_dir()
        .map_err(|_| "无法定位网络组件目录。")?
        .join(".local/state/lan-remoteapp/network"))
}
pub fn binary(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("remoteapp-network", tauri::path::BaseDirectory::Resource)
        .map_err(|_| "缺少网络组件。".into())
}
fn exchange(app: &tauri::AppHandle, body: &Value) -> Result<Value, String> {
    let mut socket = UnixStream::connect(directory(app)?.join("control.sock"))
        .map_err(|_| "网络组件未就绪。")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|_| "网络组件连接失败。")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "网络组件连接失败。")?;
    let mut bytes = serde_json::to_vec(body).map_err(|_| "请求无效。")?;
    bytes.push(b'\n');
    socket.write_all(&bytes).map_err(|_| "网络组件连接失败。")?;
    let mut line = String::new();
    BufReader::new(socket)
        .take(128 * 1024)
        .read_line(&mut line)
        .map_err(|_| "网络操作超时，请检查设备地址和配对状态。")?;
    let value: Value = serde_json::from_str(&line).map_err(|_| "网络组件响应无效。")?;
    if value.get("error").is_some() {
        return Err(
            "操作未完成。请检查邀请是否过期、设备地址和共享状态；身份变更需要先撤销旧配对。".into(),
        );
    }
    Ok(value)
}
pub fn ensure(app: &tauri::AppHandle) -> Result<(), String> {
    let _lock = START.lock().unwrap();
    if exchange(app, &json!({"op":"status"})).is_ok() {
        return Ok(());
    }
    let publisher = crate::local_node::ensure(app)?.with_file_name("node.sock");
    let info = crate::local_node::info(app)?;
    let mut child = Command::new(binary(app)?)
        .arg("serve")
        .arg("--state")
        .arg(directory(app)?)
        .arg("--publisher")
        .arg(publisher)
        .arg("--device-id")
        .arg(info["device_id"].as_str().ok_or("本机身份无效。")?)
        .arg("--name")
        .arg(info["name"].as_str().unwrap_or("Mac"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "网络组件启动失败。")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    for _ in 0..40 {
        if exchange(app, &json!({"op":"status"})).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("网络组件启动失败，请检查 UDP 42424 端口是否已占用。".into())
}
pub fn manage(app: &tauri::AppHandle, body: Value) -> Result<Value, String> {
    ensure(app)?;
    exchange(app, &body)
}
pub fn target(
    app: &tauri::AppHandle,
    device: &Device,
) -> Result<Target, remoteapp_transport::Error> {
    device.validate()?;
    if let Some(id) = &device.peer_id {
        ensure(app).map_err(|_| remoteapp_transport::Error::Connection)?;
        let value = exchange(app, &json!({"op":"status"}))
            .map_err(|_| remoteapp_transport::Error::Connection)?;
        if !value["peers"]
            .as_array()
            .is_some_and(|peers| peers.iter().any(|p| p["device_id"] == *id))
        {
            return Err(remoteapp_transport::Error::Invalid(
                "设备已解除配对，请重新配对。",
            ));
        }
        Ok(Target::Native(
            directory(app)
                .map_err(|_| remoteapp_transport::Error::Connection)?
                .join(format!("r-{id}.sock")),
        ))
    } else {
        Ok(Target::Ssh(device.ssh()))
    }
}
