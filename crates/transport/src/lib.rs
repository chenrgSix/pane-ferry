//! SSH control transport. No password handling and no client-generated remote shell commands.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const REMOTE_COMMAND: &str = ".local/bin/remoteapp-agent bridge";
const SSH: &str = "/usr/bin/ssh";
const MAX_CONTROL_LINE: usize = 1024 * 1024;
const SSH_OPTIONS: &[&str] = &[
    "-F",
    "/dev/null",
    "-T",
    "-oBatchMode=yes",
    "-oStrictHostKeyChecking=yes",
    "-oConnectTimeout=8",
    "-oConnectionAttempts=1",
    "-oClearAllForwardings=yes",
    "-oForwardAgent=no",
    "-oPermitLocalCommand=no",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub user: String,
    pub port: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Started {
    pub session_id: String,
    pub backend: String,
    pub display: String,
}
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("SSH 连接或远端 Agent 未就绪，请检查已验证的 known_hosts、SSH 登录和 Agent 服务。")]
    Connection,
    #[error("远端控制请求超时；不会自动重试启动，请先检查远端会话。")]
    Timeout,
    #[error("远端返回无效或过大的消息。")]
    Protocol,
    #[error("远端拒绝请求：{0}")]
    Remote(String),
    #[error("本地程序无法启动，请检查已安装的依赖和可执行文件路径。")]
    Local,
}
impl Endpoint {
    pub fn validate(&self) -> Result<(), Error> {
        // Narrow host grammar deliberately excludes ssh aliases/config expansion, IPv6, URLs, options.
        if self.host.is_empty()
            || self.host.len() > 253
            || !self
                .host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
            || !self.host.as_bytes()[0].is_ascii_alphanumeric()
            || !self.host.as_bytes()[self.host.len() - 1].is_ascii_alphanumeric()
        {
            return Err(Error::Invalid(
                "主机必须是 IPv4 地址或 DNS 名，不支持 URL、SSH 别名配置或 IPv6。",
            ));
        }
        if self.host.split('.').any(|label| {
            label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-')
        }) {
            return Err(Error::Invalid("主机名格式无效。"));
        }
        if self.user.is_empty()
            || self.user.len() > 64
            || !self
                .user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || self.user.starts_with('-')
        {
            return Err(Error::Invalid("SSH 用户名格式无效。"));
        }
        if self.port == 0 {
            return Err(Error::Invalid("SSH 端口必须在 1–65535。"));
        }
        Ok(())
    }
    pub fn ssh_args(&self) -> Result<Vec<String>, Error> {
        self.validate()?;
        let mut args = SSH_OPTIONS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        args.extend([
            "-p".into(),
            self.port.to_string(),
            "-l".into(),
            self.user.clone(),
            "--".into(),
            self.host.clone(),
            REMOTE_COMMAND.into(),
        ]);
        Ok(args)
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
async fn request(endpoint: &Endpoint, body: Value, expected: &str) -> Result<Value, Error> {
    let mut child = Command::new(SSH)
        .args(endpoint.ssh_args()?)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::Connection)?;
    let mut input = child.stdin.take().ok_or(Error::Connection)?;
    let mut output = BufReader::new(child.stdout.take().ok_or(Error::Connection)?);
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        input
            .write_all(b"{\"type\":\"hello\",\"version\":1}\n")
            .await
            .map_err(|_| Error::Connection)?;
        input.flush().await.map_err(|_| Error::Connection)?;
        let hello = read_message(&mut output).await?;
        if hello["type"] != "hello" {
            return Err(Error::Protocol);
        }
        let mut data = serde_json::to_vec(&body).map_err(|_| Error::Protocol)?;
        data.push(b'\n');
        input
            .write_all(&data)
            .await
            .map_err(|_| Error::Connection)?;
        input.flush().await.map_err(|_| Error::Connection)?;
        for _ in 0..64 {
            let value = read_message(&mut output).await?;
            if value["type"] == expected {
                return Ok(value);
            }
        }
        Err(Error::Protocol)
    })
    .await
    .map_err(|_| Error::Timeout)
    .and_then(|v| v);
    // Closing this control connection never implies termination of the session.
    let _ = input
        .write_all(b"{\"type\":\"session.detach\",\"version\":1}\n")
        .await;
    drop(input);
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
pub async fn catalog(endpoint: &Endpoint) -> Result<Vec<AppInfo>, Error> {
    let value = request(endpoint, json!({"type":"app.list","version":1}), "app.list").await?;
    let apps: Vec<AppInfo> =
        serde_json::from_value(value["apps"].clone()).map_err(|_| Error::Protocol)?;
    if apps.len() > 1024 {
        return Err(Error::Protocol);
    }
    for app in &apps {
        validate_id(&app.id)?;
        if app.name.len() > 1024 || app.description.len() > 4096 {
            return Err(Error::Protocol);
        }
    }
    Ok(apps)
}
pub async fn launch_xpra(
    endpoint: &Endpoint,
    app_id: &str,
    request_id: &str,
) -> Result<Started, Error> {
    launch_session(endpoint, app_id, request_id, "xpra").await
}
pub async fn launch_native(
    endpoint: &Endpoint,
    app_id: &str,
    request_id: &str,
) -> Result<Started, Error> {
    launch_session(endpoint, app_id, request_id, "native").await
}
async fn launch_session(
    endpoint: &Endpoint,
    app_id: &str,
    request_id: &str,
    backend: &str,
) -> Result<Started, Error> {
    validate_id(app_id)?;
    validate_id(request_id)?;
    let value = request(endpoint, json!({"type":"app.launch","version":1,"app_id":app_id,"request_id":request_id,"backend":backend,"attach":false}), "session.started").await?;
    parse_started(value, backend)
}
fn parse_started(value: Value, backend: &str) -> Result<Started, Error> {
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
    if value["backend"] != backend {
        return Err(Error::Protocol);
    }
    Ok(Started {
        session_id,
        backend: backend.into(),
        display: number.into(),
    })
}
pub fn xpra_args(endpoint: &Endpoint, display: &str) -> Result<Vec<String>, Error> {
    endpoint.validate()?;
    if display.is_empty() || display.len() > 6 || !display.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid("显示号无效。"));
    }
    Ok(vec![
        "attach".into(),
        format!(
            "ssh://{}@{}:{}/{}",
            endpoint.user, endpoint.host, endpoint.port, display
        ),
        format!("--ssh={} {}", SSH, SSH_OPTIONS.join(" ")),
        "--reconnect=no".into(),
        "--file-transfer=no".into(),
        "--printing=no".into(),
        "--open-files=no".into(),
        "--open-url=no".into(),
        "--webcam=no".into(),
        "--speaker=off".into(),
        "--microphone=off".into(),
        "--clipboard=no".into(),
    ])
}
pub fn spawn_xpra(endpoint: &Endpoint, display: &str, executable: &Path) -> Result<u32, Error> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err(Error::Invalid("请提供已安装 Xpra 的绝对可执行文件路径。"));
    }
    let mut child = std::process::Command::new(executable)
        .args(xpra_args(endpoint, display)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::Local)?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint() -> Endpoint {
        Endpoint {
            host: "192.168.1.20".into(),
            user: "alice".into(),
            port: 22,
        }
    }
    #[test]
    fn refuses_injection() {
        for host in [
            "-oProxyCommand=evil",
            "a;id",
            "a\nb",
            "$(id)",
            "a b",
            "a@b",
            "a..b",
            "[::1]",
        ] {
            let mut e = endpoint();
            e.host = host.into();
            assert!(e.validate().is_err(), "{host}");
        }
    }
    #[test]
    fn refuses_bad_ids() {
        for id in ["", "-x", "app;id", "app\n", "$(id)", "a/b"] {
            assert!(validate_id(id).is_err());
        }
    }
    #[test]
    fn fixes_remote_command() {
        let args = endpoint().ssh_args().unwrap();
        assert_eq!(args.last().unwrap(), REMOTE_COMMAND);
        assert!(args.contains(&"-oStrictHostKeyChecking=yes".into()));
        assert!(args.contains(&"/dev/null".into()));
    }
    #[test]
    fn disables_xpra_extras() {
        let args = xpra_args(&endpoint(), "120").unwrap();
        assert!(args.iter().any(|a| a.starts_with("--ssh=/usr/bin/ssh ")));
        assert!(args.contains(&"--file-transfer=no".into()));
        assert!(xpra_args(&endpoint(), "120;id").is_err());
    }
    #[tokio::test]
    async fn rejects_wrong_version() {
        let mut r = BufReader::new(&b"{\"type\":\"hello\",\"version\":99}\n"[..]);
        assert!(read_message(&mut r).await.is_err());
    }
    #[test]
    fn validates_prepared_native_session() {
        let value = json!({"session_id":"abc123","backend":"native","display":101});
        let session = parse_started(value.clone(), "native").unwrap();
        assert_eq!(session.session_id, "abc123");
        assert_eq!(session.display, "101");
        assert!(parse_started(value, "xpra").is_err());
        assert!(parse_started(
            json!({"session_id":"abc123","backend":"native","display":"101;id"}),
            "native"
        )
        .is_err());
    }
}
