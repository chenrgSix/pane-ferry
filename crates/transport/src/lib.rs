//! Bounded RWP control exchanges over device-scoped local QUIC routes.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MAX_CONTROL_LINE: usize = 16 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
    #[serde(default)]
    pub icon_png: Option<String>,
    #[serde(default = "custom_source")]
    pub source: String,
}
fn custom_source() -> String {
    "custom".into()
}
#[derive(Debug, Clone, Serialize)]
pub struct AppCatalog {
    pub apps: Vec<AppInfo>,
    pub discovery_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopWindowInfo {
    pub id: String,
    #[serde(default)]
    pub app_id: Option<String>,
    pub window_id: u32,
    pub name: String,
    pub title: String,
    pub icon_png: Option<String>,
    pub minimized: bool,
    pub controlled: bool,
    #[serde(default)]
    pub session_id: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct DesktopWindows {
    pub windows: Vec<DesktopWindowInfo>,
    pub unavailable_reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopMonitor {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct DesktopMonitors {
    pub monitors: Vec<DesktopMonitor>,
    pub unavailable_reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Started {
    pub session_id: String,
    pub backend: String,
    pub display: String,
    pub scale_factor: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub app_id: String,
    pub backend: String,
    pub display: u16,
    pub controlled: bool,
    #[serde(default)]
    pub window_count: Option<usize>,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub monitor_id: Option<String>,
}
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("连接暂时中断，请等待网络恢复。")]
    Connection,
    #[error("设备已解除配对，请重新配对。")]
    PairingRequired,
    #[error("远端控制请求超时；不会自动重试启动，请先检查远端会话。")]
    Timeout,
    #[error("远端返回无效或过大的消息。")]
    Protocol,
    #[error("远端拒绝请求：{}", remote_error_message(.0))]
    Remote(String),
}
pub fn remote_error_message(code: &str) -> String {
    match code {
        "application_scene_unavailable" => "当前来源不支持应用会话画面。首版需要新版 Ubuntu 扩展和已打开的 GNOME/Wayland 应用。".into(),
        "session_mode_conflict" => "已有会话采用不同的显示方式，请先断开该会话。".into(),
        "sharing_disabled" => "对方已停止本机共享，应用仍在原设备运行。".into(),
        "screen_recording_required" => "请在 Mac 允许 窗渡 PaneFerry 的录屏权限。".into(),
        "accessibility_required" => "请在 Mac 允许 窗渡 PaneFerry 的辅助功能权限。".into(),
        "desktop_locked" => "远端桌面已锁定，解锁后可重新连接。".into(),
        "desktop_monitor_unavailable" => "远端当前没有可共享的显示器，请确认 Ubuntu 已登录桌面并刷新。".into(),
        "desktop_capture_unavailable" => "远端桌面画面暂不可用，请检查 GNOME 录屏与 PipeWire；原桌面未被修改。".into(),
        "desktop_geometry_fixed" => "整桌面模式保持远端显示器原尺寸，只在本机等比显示。".into(),
        "desktop_close_unavailable" => "整桌面窗口只能断开连接，远端桌面与应用继续运行。".into(),
        "wayland_bridge_unavailable" => "应用正在 Wayland 桌面运行。请在 Ubuntu 启用窗渡 GNOME 扩展，再刷新运行窗口；不会另建浏览器配置。".into(),
        "wayland_bridge_failed" => "Wayland 窗口接口暂不可用，请检查 Ubuntu 的窗渡 GNOME 扩展。".into(),
        "wayland_bridge_outdated" => "Ubuntu 仍在运行旧版窗口扩展。保存远端工作后，注销并重新登录以启用新版；当前应用不会自动关闭。".into(),
        "wayland_capture_unavailable" => "Wayland 窗口采集未就绪，请检查 PipeWire 与 GStreamer 组件。".into(),
        "resolution_limited" => "请求尺寸超出当前 Wayland 窗口采集范围。请缩小窗口或选择较低分辨率；原应用和桌面缩放保持不变。".into(),
        "window_size_unavailable" => "应用窗口的尺寸约束无法在当前采集范围内满足，原会话已保留。".into(),
        "window_resize_failed" => "应用窗口未接受尺寸调整，请调整窗口设置后重试；原会话已保留。".into(),
        "wayland_binary_clipboard_unavailable" => "当前 Wayland 窗口仅支持文字剪贴板。".into(),
        "wayland_cut_unavailable" => "当前 Wayland 窗口暂不支持远程剪切，请使用复制；远端内容未删除。".into(),
        "wayland_text_unavailable" => "当前输入框不支持直接输入，请使用粘贴；输入未自动重试。".into(),
        "application_multiple_windows" => "该应用已有多个窗口，请从“运行中的窗口”选择要连接的窗口。".into(),
        "application_start_failed" => "应用在产生窗口前已退出，可能已转交给原桌面的实例。请刷新运行窗口；没有可连接窗口时检查应用兼容性。".into(),
        "application_window_pending" => "应用尚未产生窗口，会话已保留。可稍后刷新运行状态或重试，不会重复启动。".into(),
        "existing_process_unavailable" => "原窗口对应的进程已退出，请刷新窗口列表。".into(),
        "existing_process_mismatch" => "原窗口绑定已失效，请在 Ubuntu 重新确认应用进程。".into(),
        "existing_window_unavailable" => {
            "该应用目前没有可接入的窗口。请先在 Ubuntu 打开窗口并刷新运行状态。".into()
        }
        "session_expired" | "session_exited" => "上次会话已结束。再次打开将创建新会话。".into(),
        "session_controlled" => "该会话已有控制端，请先断开原控制端。".into(),
        "request_id_conflict" => "启动参数已变化，请重新打开应用。".into(),
        "app_not_approved" => "应用或窗口入口已失效，请重新读取列表。".into(),
        "desktop_application_unavailable" => "应用已变更或卸载，请刷新应用列表。".into(),
        "desktop_secrets_unavailable" => {
            "应用需要 Ubuntu 桌面密钥服务，请确认桌面已登录且已安装 xdg-dbus-proxy。".into()
        }
        "desktop_activation_unavailable" | "desktop_command_unsupported" => {
            "该应用的启动方式暂未支持。".into()
        }
        "desktop_executable_unavailable" => "该应用的程序文件不可用，请在 Ubuntu 检查安装。".into(),
        "desktop_terminal_unavailable" => "Ubuntu 缺少用于启动此应用的 xterm 终端。".into(),
        "desktop_working_directory_unavailable" => {
            "该应用的启动目录已不存在，请在 Ubuntu 检查安装。".into()
        }
        "application_discovery_unavailable" => {
            "Ubuntu 应用发现组件未就绪，请检查 Python GI、GTK 和 GIO 运行依赖。".into()
        }
        "agent_upgrade_required" => "远端 Agent 需要更新，当前仅能读取自定义入口。".into(),
        "desktop_unavailable" => "未找到可连接的 Ubuntu 桌面，请确认用户已登录图形桌面。".into(),
        "session_limit" => "运行中的会话已达上限，请保存工作并关闭不用的远端应用。".into(),
        "request_history_limit" => "启动记录已达上限，请保存远端工作后重启 Agent。".into(),
        _ => code
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(64)
            .collect(),
    }
}
impl Error {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PairingRequired => "pairing_required",
            Self::Connection | Self::Timeout => "connection",
            Self::Invalid(_) => "invalid",
            Self::Protocol => "protocol",
            Self::Remote(_) => "remote",
        }
    }
    /// Only explicit rejections prove it is safe to discard an idempotency key.
    pub fn retry_same_request(&self) -> bool {
        match self {
            Error::Invalid(_) | Error::PairingRequired => false,
            Error::Remote(code) => !matches!(
                code.as_str(),
                "existing_process_unavailable"
                    | "existing_process_mismatch"
                    | "existing_window_unavailable"
                    | "application_multiple_windows"
                    | "application_start_failed"
                    | "wayland_bridge_unavailable"
                    | "wayland_bridge_outdated"
                    | "wayland_capture_unavailable"
                    | "session_expired"
                    | "session_exited"
                    | "request_id_conflict"
                    | "app_not_approved"
                    | "desktop_application_unavailable"
                    | "desktop_secrets_unavailable"
                    | "session_limit"
                    | "request_history_limit"
            ),
            _ => true,
        }
    }
}
pub fn validate_id(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        || id.starts_with('-')
    {
        return Err(Error::Invalid("应用或会话 ID 格式无效。"));
    }
    Ok(())
}
async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<Value, Error> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await.map_err(|_| Error::Connection)?;
        if buffer.is_empty() {
            return Err(Error::Connection);
        }
        let amount = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(buffer.len());
        if line.len() + amount > MAX_CONTROL_LINE {
            return Err(Error::Protocol);
        }
        let ended = buffer[amount - 1] == b'\n';
        line.extend_from_slice(&buffer[..amount]);
        reader.consume(amount);
        if ended {
            break;
        }
    }
    let value: Value = serde_json::from_slice(&line).map_err(|_| Error::Protocol)?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(Error::Protocol);
    }
    if value.get("type").and_then(Value::as_str) == Some("error") {
        // Return bounded code, not arbitrary remote error text or private command output.
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("remote_error");
        let safe: String = code
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(64)
            .collect();
        return Err(Error::Remote(safe));
    }
    Ok(value)
}
/// Private device route supplied by the local network service, never by a peer.
#[derive(Debug, Clone)]
pub struct Target {
    pub socket: PathBuf,
}
async fn exchange<R, W>(
    output: &mut R,
    input: &mut W,
    body: Value,
    expected: &str,
) -> Result<Value, Error>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    input
        .write_all(b"{\"type\":\"hello\",\"version\":1}\n")
        .await
        .map_err(|_| Error::Connection)?;
    input.flush().await.map_err(|_| Error::Connection)?;
    let hello = read_message(output).await?;
    if hello["type"] != "hello" {
        return Err(Error::Protocol);
    }
    if body.get("scene").is_some_and(|v| !v.is_null())
        && !hello["capabilities"]
            .as_array()
            .is_some_and(|caps| caps.iter().any(|c| c == "application.scene"))
    {
        return Err(Error::Remote("application_scene_unavailable".into()));
    }
    let mut data = serde_json::to_vec(&body).map_err(|_| Error::Protocol)?;
    if data.len() >= MAX_CONTROL_LINE {
        return Err(Error::Protocol);
    }
    data.push(b'\n');
    input
        .write_all(&data)
        .await
        .map_err(|_| Error::Connection)?;
    input.flush().await.map_err(|_| Error::Connection)?;
    for _ in 0..64 {
        let value = read_message(output).await?;
        if value["type"] == expected {
            return Ok(value);
        }
    }
    Err(Error::Protocol)
}
async fn request(endpoint: &Target, body: Value, expected: &str) -> Result<Value, Error> {
    tokio::time::timeout(Duration::from_secs(25), async {
        let socket = paneferry_platform::ipc::LocalStream::connect(&endpoint.socket)
            .await
            .map_err(|_| Error::Connection)?;
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let result = exchange(&mut read, &mut write, body, expected).await;
        let _ = write.shutdown().await;
        result
    })
    .await
    .map_err(|_| Error::Timeout)?
}
pub async fn catalog(endpoint: &Target) -> Result<AppCatalog, Error> {
    let value = request(endpoint, json!({"type":"app.list","version":1}), "app.list").await?;
    let mut apps: Vec<AppInfo> =
        serde_json::from_value(value["apps"].clone()).map_err(|_| Error::Protocol)?;
    if apps.len() > 1024 {
        return Err(Error::Protocol);
    }
    for app in &mut apps {
        validate_id(&app.id)?;
        if app.name.len() > 1024 || app.description.len() > 4096 {
            return Err(Error::Protocol);
        }
        if !matches!(app.source.as_str(), "desktop" | "custom") {
            return Err(Error::Protocol);
        }
        validate_icon(app.icon_png.as_deref())?;
        if let Some(code) = &app.unavailable_reason {
            app.unavailable_reason = Some(remote_error_message(code));
        }
    }
    let warning = if value["discovery_supported"] != true {
        Some("agent_upgrade_required")
    } else {
        value["discovery_error"].as_str()
    };
    Ok(AppCatalog {
        apps,
        discovery_error: warning.map(remote_error_message),
    })
}
fn validate_icon(icon: Option<&str>) -> Result<(), Error> {
    let Some(icon) = icon else {
        return Ok(());
    };
    if icon.len() > 24 * 1024 {
        return Err(Error::Protocol);
    }
    let bytes = STANDARD.decode(icon).map_err(|_| Error::Protocol)?;
    if bytes.len() < 24
        || bytes.len() > 16 * 1024
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || &bytes[12..16] != b"IHDR"
    {
        return Err(Error::Protocol);
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || width > 128 || height > 128 {
        return Err(Error::Protocol);
    }
    Ok(())
}
pub async fn desktop_windows(endpoint: &Target) -> Result<DesktopWindows, Error> {
    let value = request(
        endpoint,
        json!({"type":"window.list","version":1}),
        "window.list",
    )
    .await?;
    let windows: Vec<DesktopWindowInfo> =
        serde_json::from_value(value["windows"].clone()).map_err(|_| Error::Protocol)?;
    if windows.len() > 128 {
        return Err(Error::Protocol);
    }
    for window in &windows {
        validate_id(&window.id)?;
        if let Some(app) = &window.app_id {
            validate_id(app)?;
        }
        if let Some(session) = &window.session_id {
            validate_id(session)?;
        }
        if window.window_id == 0 || window.name.len() > 1024 || window.title.len() > 1024 {
            return Err(Error::Protocol);
        }
        validate_icon(window.icon_png.as_deref())?;
    }
    Ok(DesktopWindows {
        windows,
        unavailable_reason: value["unavailable_reason"]
            .as_str()
            .map(remote_error_message),
    })
}
pub async fn desktop_monitors(endpoint: &Target) -> Result<DesktopMonitors, Error> {
    let value = request(
        endpoint,
        json!({"type":"desktop.list","version":1}),
        "desktop.list",
    )
    .await?;
    let monitors: Vec<DesktopMonitor> =
        serde_json::from_value(value["monitors"].clone()).map_err(|_| Error::Protocol)?;
    if monitors.len() > 16 {
        return Err(Error::Protocol);
    }
    for monitor in &monitors {
        validate_id(&monitor.id)?;
        if monitor.name.is_empty()
            || monitor.name.len() > 128
            || monitor.width == 0
            || monitor.height == 0
            || monitor.width > 8192
            || monitor.height > 8192
        {
            return Err(Error::Protocol);
        }
    }
    Ok(DesktopMonitors {
        monitors,
        unavailable_reason: value["unavailable_reason"]
            .as_str()
            .map(remote_error_message),
    })
}
pub async fn sessions(endpoint: &Target) -> Result<Vec<SessionInfo>, Error> {
    let value = request(
        endpoint,
        json!({"type":"session.list","version":1}),
        "session.list",
    )
    .await?;
    let sessions: Vec<SessionInfo> =
        serde_json::from_value(value["sessions"].clone()).map_err(|_| Error::Protocol)?;
    if sessions.len() > 16 {
        return Err(Error::Protocol);
    }
    for session in &sessions {
        validate_id(&session.session_id)?;
        validate_id(&session.app_id)?;
        if session.backend != "native" {
            return Err(Error::Protocol);
        }
        if !matches!(
            session.mode.as_deref(),
            None | Some("app") | Some("desktop") | Some("scene")
        ) {
            return Err(Error::Protocol);
        }
        if let Some(monitor_id) = &session.monitor_id {
            validate_id(monitor_id)?;
            if session.mode.as_deref() != Some("desktop") {
                return Err(Error::Protocol);
            }
        }
    }
    Ok(sessions)
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSize {
    pub width: u32,
    pub height: u32,
}
pub async fn launch_native(
    endpoint: &Target,
    app_id: &str,
    request_id: &str,
    scale_factor: u8,
    scene: Option<SceneSize>,
) -> Result<Started, Error> {
    validate_id(app_id)?;
    validate_id(request_id)?;
    if !matches!(scale_factor, 1 | 2) {
        return Err(Error::Invalid("清晰度缩放必须为 1 或 2。"));
    }
    let mut body = json!({"type":"app.launch","version":1,"app_id":app_id,"request_id":request_id,"backend":"native","attach":false,"scale_factor":scale_factor});
    if let Some(scene) = scene {
        if scene.width < 640
            || scene.height < 480
            || scene.width > 4096
            || scene.height > 4096
            || u64::from(scene.width) * u64::from(scene.height) > 8_000_000
            || scene.width % u32::from(scale_factor) != 0
            || scene.height % u32::from(scale_factor) != 0
        {
            return Err(Error::Invalid("应用会话画面尺寸无效。"));
        }
        body["scene"] = serde_json::to_value(scene).map_err(|_| Error::Protocol)?;
    }
    let value = request(endpoint, body, "session.started").await?;
    parse_started(value)
}
pub async fn start_desktop(
    endpoint: &Target,
    monitor_id: &str,
    request_id: &str,
) -> Result<Started, Error> {
    validate_id(monitor_id)?;
    validate_id(request_id)?;
    let value = request(
        endpoint,
        json!({"type":"desktop.start","version":1,"monitor_id":monitor_id,"request_id":request_id}),
        "session.started",
    )
    .await?;
    if value["mode"] != "desktop" {
        return Err(Error::Protocol);
    }
    parse_started(value)
}
fn parse_started(value: Value) -> Result<Started, Error> {
    let scale_factor = match value.get("scale_factor") {
        None => 1,
        Some(value) => match value.as_u64() {
            Some(1) => 1,
            Some(2) => 2,
            _ => return Err(Error::Protocol),
        },
    };
    let display = match &value["display"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(Error::Protocol),
    };
    let number = display.trim_start_matches(':');
    if number.is_empty() || number.len() > 6 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Protocol);
    }
    let session_id = value["session_id"]
        .as_str()
        .ok_or(Error::Protocol)?
        .to_string();
    validate_id(&session_id)?;
    if value["backend"] != "native" {
        return Err(Error::Protocol);
    }
    Ok(Started {
        session_id,
        backend: "native".into(),
        display: number.into(),
        scale_factor,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_bounded_png_icons_and_rejects_active_or_oversized_data() {
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend_from_slice(&64u32.to_be_bytes());
        header.extend_from_slice(&96u32.to_be_bytes());
        assert!(validate_icon(Some(&STANDARD.encode(&header))).is_ok());
        header[16..20].copy_from_slice(&4096u32.to_be_bytes());
        assert!(validate_icon(Some(&STANDARD.encode(&header))).is_err());
        assert!(validate_icon(Some(&STANDARD.encode(b"<svg onload='evil()'/>"))).is_err());
        assert!(validate_icon(Some(&"A".repeat(24 * 1024 + 1))).is_err());
    }
    #[tokio::test]
    async fn stale_process_errors_are_actionable_without_private_details() {
        let mut reader = BufReader::new(&b"{\"type\":\"error\",\"version\":1,\"code\":\"existing_process_unavailable\",\"message\":\"PRIVATE COMMAND\"}\n"[..]);
        let error = read_message(&mut reader).await.unwrap_err();
        assert!(error.to_string().contains("进程已退出"));
        assert!(!error.to_string().contains("PRIVATE"));
        assert!(!error.retry_same_request());
        for code in ["session_exited", "session_expired", "request_id_conflict"] {
            assert!(!Error::Remote(code.into()).retry_same_request());
        }
        assert!(Error::Remote("request_failed".into()).retry_same_request());
        assert!(Error::Timeout.retry_same_request());
        assert!(Error::Connection.retry_same_request());
    }
    #[test]
    fn refuses_bad_ids() {
        for id in ["", "-x", "app;id", "app\n", "$(id)", "a/b"] {
            assert!(validate_id(id).is_err());
        }
    }
    #[tokio::test]
    async fn rejects_wrong_version() {
        let mut r = BufReader::new(&b"{\"type\":\"hello\",\"version\":99}\n"[..]);
        assert!(read_message(&mut r).await.is_err());
    }
    #[test]
    fn validates_prepared_native_session() {
        let value = json!({"session_id":"abc123","backend":"native","display":101});
        let session = parse_started(value.clone()).unwrap();
        assert_eq!(session.session_id, "abc123");
        assert_eq!(session.display, "101");
        assert_eq!(
            parse_started(json!({"session_id":"desktop","backend":"native","display":0}))
                .unwrap()
                .display,
            "0"
        );
        assert!(
            parse_started(json!({"session_id":"abc123","backend":"external","display":101}))
                .is_err()
        );
        assert!(parse_started(
            json!({"session_id":"abc123","backend":"native","display":"101;id"})
        )
        .is_err());
    }
    #[test]
    fn keeps_visible_window_count_for_launch_status() {
        let session: SessionInfo = serde_json::from_value(json!({
            "session_id":"starting","app_id":"feishu","backend":"native",
            "display":149,"controlled":false,"window_count":0
        }))
        .unwrap();
        assert_eq!(session.window_count, Some(0));
        let ready: SessionInfo = serde_json::from_value(json!({
            "session_id":"ready","app_id":"feishu","backend":"native",
            "display":149,"controlled":false,"window_count":1
        }))
        .unwrap();
        assert_eq!(ready.window_count, Some(1));
    }
}

#[cfg(test)]
mod unified_transport_tests {
    use super::*;
    #[tokio::test]
    async fn session_catalog_accepts_scene_alongside_app_and_desktop_modes() {
        let directory = std::env::temp_dir().join(format!("rws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("route.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let legacy = json!({"session_id":"legacy","app_id":"editor","backend":"native",
            "display":0,"controlled":false});
        let with_mode = |mode: &str| {
            let mut value = legacy.clone();
            value["mode"] = json!(mode);
            value["session_id"] = json!(mode);
            value
        };
        let app = with_mode("app");
        let scene = with_mode("scene");
        let mut desktop = with_mode("desktop");
        desktop["monitor_id"] = json!("DP-1");
        let mut unknown = scene.clone();
        unknown["mode"] = json!("unknown");
        let mut wrong_monitor = scene.clone();
        wrong_monitor["monitor_id"] = json!("DP-1");
        let responses = [
            json!([legacy, app, scene, desktop]),
            json!([unknown]),
            json!([wrong_monitor]),
        ];
        let server = tokio::spawn(async move {
            for sessions in responses {
                let (socket, _) = listener.accept().await.unwrap();
                let (read, mut write) = socket.into_split();
                let mut read = BufReader::new(read);
                assert_eq!(read_message(&mut read).await.unwrap()["type"], "hello");
                write
                    .write_all(b"{\"version\":1,\"type\":\"hello\"}\n")
                    .await
                    .unwrap();
                assert_eq!(
                    read_message(&mut read).await.unwrap()["type"],
                    "session.list"
                );
                let response = json!({"version":1,"type":"session.list","sessions":sessions});
                write
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let target = Target { socket: path };
        let catalog = sessions(&target).await;
        assert!(matches!(sessions(&target).await, Err(Error::Protocol)));
        assert!(matches!(sessions(&target).await, Err(Error::Protocol)));
        server.await.unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        let catalog =
            catalog.expect("scene sessions must not invalidate the entire launcher catalog");
        assert_eq!(catalog.len(), 4);
        assert_eq!(catalog[2].mode.as_deref(), Some("scene"));
    }
    #[tokio::test]
    async fn scene_request_is_never_sent_to_publishers_without_the_capability() {
        for supported in [false, true] {
            let (client, server) = tokio::io::duplex(4096);
            let (read, mut write) = tokio::io::split(client);
            let mut read = BufReader::new(read);
            let task = tokio::spawn(async move {
                let (read, mut write) = tokio::io::split(server);
                let mut read = BufReader::new(read);
                assert_eq!(read_message(&mut read).await.unwrap()["type"], "hello");
                let hello = json!({"version":1,"type":"hello","capabilities":if supported {vec!["application.scene"]} else {vec![]}});
                write
                    .write_all(format!("{}\n", hello).as_bytes())
                    .await
                    .unwrap();
                if supported {
                    assert_eq!(
                        read_message(&mut read).await.unwrap()["scene"]["width"],
                        1600
                    );
                    write
                        .write_all(b"{\"version\":1,\"type\":\"session.started\"}\n")
                        .await
                        .unwrap();
                } else {
                    let mut data = String::new();
                    assert_eq!(
                        read.read_line(&mut data).await.unwrap(),
                        0,
                        "launch leaked before capability check"
                    );
                }
            });
            let result = exchange(
                &mut read,
                &mut write,
                json!({"version":1,"type":"app.launch","scene":{"width":1600,"height":900}}),
                "session.started",
            )
            .await;
            if supported {
                assert!(result.is_ok());
            } else {
                assert!(
                    matches!(result, Err(Error::Remote(code)) if code == "application_scene_unavailable")
                );
            }
            drop(write);
            drop(read);
            task.await.unwrap();
        }
    }
    #[tokio::test]
    async fn native_target_uses_same_handshake_errors_and_validation() {
        let directory = std::env::temp_dir().join(format!("rwp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("route.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut read = BufReader::new(read);
            assert_eq!(read_message(&mut read).await.unwrap()["type"], "hello");
            write
                .write_all(b"{\"version\":1,\"type\":\"hello\"}\n")
                .await
                .unwrap();
            assert_eq!(read_message(&mut read).await.unwrap()["type"], "app.list");
            write
                .write_all(b"{\"version\":1,\"type\":\"error\",\"code\":\"sharing_disabled\"}\n")
                .await
                .unwrap();
        });
        assert!(
            matches!(catalog(&Target { socket: path }).await, Err(Error::Remote(code)) if code == "sharing_disabled")
        );
        server.await.unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
