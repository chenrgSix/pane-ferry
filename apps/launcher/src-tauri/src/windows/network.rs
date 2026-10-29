use paneferry_transport::Target;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    #[serde(rename = "peerId")]
    pub peer_id: String,
}
impl Device {
    pub fn validate(&self) -> Result<(), paneferry_transport::Error> {
        if uuid::Uuid::parse_str(&self.peer_id).is_ok_and(|v| v.to_string() == self.peer_id) {
            Ok(())
        } else {
            Err(paneferry_transport::Error::Invalid("设备身份无效。"))
        }
    }
    pub fn identity(&self) -> Value {
        json!({"transport":"quic","device_id":self.peer_id})
    }
}
pub fn directory(_app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(crate::local_node::root()?.join("network"))
}
pub fn ensure(app: &tauri::AppHandle) -> Result<(), String> {
    crate::local_node::ensure(app)?;
    Ok(())
}
pub fn manage(app: &tauri::AppHandle, body: Value) -> Result<Value, String> {
    ensure(app)?;
    crate::local_rpc::exchange(&directory(app)?.join("control.sock"), body, false)
}
pub fn target(
    app: &tauri::AppHandle,
    device: &Device,
) -> Result<Target, paneferry_transport::Error> {
    device.validate()?;
    let status =
        manage(app, json!({"op":"status"})).map_err(|_| paneferry_transport::Error::Connection)?;
    if !status["peers"]
        .as_array()
        .is_some_and(|peers| peers.iter().any(|p| p["device_id"] == device.peer_id))
    {
        return Err(paneferry_transport::Error::PairingRequired);
    }
    Ok(Target {
        socket: directory(app)
            .map_err(|_| paneferry_transport::Error::Connection)?
            .join(format!("r-{}.sock", device.peer_id)),
    })
}
