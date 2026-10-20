use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
pub const MAX_LINE: usize = 16 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 8_000_000;
pub const MAX_TEXT_BYTES: usize = 8 * 1024;
#[derive(Debug, Deserialize)]
pub enum LaunchBackend {
    #[serde(rename = "native")]
    Native,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "hello")]
    Hello { version: u32 },
    #[serde(rename = "app.list")]
    List {},
    #[serde(rename = "app.launch")]
    Launch {
        app_id: String,
        request_id: String,
        backend: LaunchBackend,
        #[serde(default = "attach_by_default")]
        attach: bool,
        #[serde(default = "default_scale")]
        scale_factor: u8,
    },
    #[serde(rename = "session.resume")]
    Resume { session_id: String },
    #[serde(rename = "session.list")]
    Sessions {},
    #[serde(rename = "session.detach")]
    Detach {},
    #[serde(rename = "window.configure")]
    Configure {
        window_id: u32,
        width: u32,
        height: u32,
    },
    #[serde(rename = "window.focus")]
    Focus { window_id: u32 },
    #[serde(rename = "window.close.request")]
    Close { window_id: u32 },
    #[serde(rename = "input.pointer")]
    Pointer {
        window_id: u32,
        x: i32,
        y: i32,
        button: u8,
        pressed: bool,
    },
    #[serde(rename = "input.key")]
    Key {
        window_id: u32,
        keysym: u32,
        pressed: bool,
    },
    #[serde(rename = "input.text.commit")]
    Text { window_id: u32, text: String },
    #[serde(rename = "clipboard.paste")]
    Paste { window_id: u32, text: String },
    #[serde(rename = "clipboard.copy")]
    Copy { window_id: u32, request_id: String },
}
fn attach_by_default() -> bool {
    true
}
pub fn default_scale() -> u8 {
    1
}
pub fn validate_scale(scale: u8) -> Result<()> {
    if !matches!(scale, 1 | 2) {
        bail!("scale_factor must be 1 or 2")
    }
    Ok(())
}
pub fn parse_request(data: &[u8]) -> Result<Request> {
    let mut v: serde_json::Value = serde_json::from_slice(data)?;
    if v.get("version").and_then(|v| v.as_u64()) != Some(1) {
        bail!("protocol version 1 required")
    }
    if v.get("type").and_then(|v| v.as_str()) != Some("hello") {
        v.as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("object required"))?
            .remove("version");
    }
    let request = serde_json::from_value(v)?;
    if let Request::Launch { scale_factor, .. } = &request {
        validate_scale(*scale_factor)?;
    }
    if let Request::Text { text, .. } | Request::Paste { text, .. } = &request {
        validate_text(text)?;
    }
    if let Request::Copy { request_id, .. } = &request {
        if !identifier(request_id) {
            bail!("invalid copy request id")
        }
    }
    Ok(request)
}
pub fn validate_text(text: &str) -> Result<()> {
    if text.is_empty() || text.len() > MAX_TEXT_BYTES || text.contains('\0') {
        bail!("invalid or oversized controller text")
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub window_id: u32,
    pub parent_id: Option<u32>,
    pub title: String,
    pub role: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_scale")]
    pub scale_factor: u8,
    pub geometry_revision: u64,
    #[serde(default = "attach_by_default")]
    pub visible: bool,
}
pub fn dimensions(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > 4096 || h > 4096 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        bail!("invalid dimensions")
    };
    Ok(())
}
pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
/// Bounded line decoder: never allows an untrusted peer to grow an unbounded Vec.
pub fn read_line<R: BufRead>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    read_line_limit(reader, MAX_LINE)
}
pub fn read_line_limit<R: BufRead>(reader: &mut R, max: usize) -> Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            if line.is_empty() {
                return Ok(None);
            } else {
                bail!("truncated message")
            }
        }
        let take = buf
            .iter()
            .position(|b| *b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(buf.len());
        if line.len() + take > max {
            bail!("message too large")
        }
        let done = buf[take - 1] == b'\n';
        line.extend_from_slice(&buf[..take]);
        reader.consume(take);
        if done {
            return Ok(Some(line));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds() {
        assert!(dimensions(0, 2).is_err());
        assert!(dimensions(4096, 4096).is_err());
        assert!(dimensions(1920, 1080).is_ok());
    }
    #[test]
    fn ids() {
        for x in ["", "a;id", "-a\n", "a/b"] {
            assert!(!identifier(x))
        }
        assert!(identifier("mousepad-1"));
    }
    #[test]
    fn bounded() {
        let mut c = std::io::Cursor::new(b"{}\n");
        assert_eq!(read_line(&mut c).unwrap().unwrap(), b"{}\n");
        assert!(read_line(&mut c).unwrap().is_none());
    }
    #[test]
    fn versions() {
        assert!(parse_request(br#"{"type":"app.list","version":1}"#).is_ok());
        assert!(parse_request(br#"{"type":"app.list","version":2}"#).is_err());
        assert!(parse_request(br#"{"type":"app.list"}"#).is_err());
    }
    #[test]
    fn rejects_fields() {
        assert!(serde_json::from_str::<Request>(r#"{"type":"app.list","command":"id"}"#).is_err());
    }
    #[test]
    fn rejects_oversized_and_truncated_lines() {
        let mut over = std::io::Cursor::new(b"12345\n");
        assert!(read_line_limit(&mut over, 5).is_err());
        let mut exact = std::io::Cursor::new(b"1234\n");
        assert_eq!(read_line_limit(&mut exact, 5).unwrap().unwrap(), b"1234\n");
        let mut partial = std::io::Cursor::new(b"{}");
        assert!(read_line_limit(&mut partial, 100).is_err());
    }
    #[test]
    fn rejects_unknown_messages_and_extra_envelope() {
        for data in [
            r#"{"type":"shell.execute","version":1}"#,
            r#"{"type":"input.key","version":1,"window_id":1,"keysym":65,"pressed":true,"session_id":"extra"}"#,
            r#"{"type":"hello","version":true}"#,
            r#"{"type":"hello","version":1,"command":"id"}"#,
        ] {
            assert!(parse_request(data.as_bytes()).is_err());
        }
    }
    #[test]
    fn accepts_native_input_contract() {
        for data in [
            r#"{"type":"input.key","version":1,"window_id":1,"keysym":65,"pressed":true}"#,
            r#"{"type":"input.pointer","version":1,"window_id":1,"x":10,"y":20,"button":1,"pressed":false}"#,
            r#"{"type":"window.configure","version":1,"window_id":1,"width":960,"height":640}"#,
            r#"{"type":"session.resume","version":1,"session_id":"abc123"}"#,
        ] {
            assert!(parse_request(data.as_bytes()).is_ok());
        }
    }
    #[test]
    fn launch_can_prepare_without_attaching_and_keeps_old_default() {
        let old = parse_request(br#"{"type":"app.launch","version":1,"app_id":"a","request_id":"r","backend":"native"}"#).unwrap();
        assert!(matches!(old, Request::Launch { attach: true, .. }));
        let prepared = parse_request(br#"{"type":"app.launch","version":1,"app_id":"a","request_id":"r","backend":"native","attach":false}"#).unwrap();
        assert!(matches!(prepared, Request::Launch { attach: false, .. }));
    }
    #[test]
    fn launch_scale_is_optional_and_bounded() {
        let message = |scale| serde_json::json!({"type":"app.launch","version":1,"app_id":"a","request_id":"r","backend":"native","scale_factor":scale});
        for scale in [1, 2] {
            assert!(parse_request(&serde_json::to_vec(&message(scale)).unwrap()).is_ok());
        }
        for scale in [0, 3, 255] {
            assert!(parse_request(&serde_json::to_vec(&message(scale)).unwrap()).is_err());
        }
        let old = parse_request(br#"{"type":"app.launch","version":1,"app_id":"a","request_id":"r","backend":"native"}"#).unwrap();
        assert!(matches!(
            old,
            Request::Launch {
                scale_factor: 1,
                ..
            }
        ));
    }
    #[test]
    fn rejects_non_native_launch_backend() {
        let request = serde_json::json!({"type":"app.launch","version":1,"app_id":"a","request_id":"r","backend":"external"});
        assert!(parse_request(&serde_json::to_vec(&request).unwrap()).is_err());
    }
    #[test]
    fn controller_text_and_explicit_copy_are_bounded() {
        for kind in ["input.text.commit", "clipboard.paste"] {
            let message = serde_json::json!({"type":kind,"version":1,"window_id":1,"text":"中文🙂\ncontroller"});
            assert!(parse_request(&serde_json::to_vec(&message).unwrap()).is_ok());
            for text in [String::new(), "a\0b".into(), "x".repeat(MAX_TEXT_BYTES + 1)] {
                let message =
                    serde_json::json!({"type":kind,"version":1,"window_id":1,"text":text});
                assert!(parse_request(&serde_json::to_vec(&message).unwrap()).is_err());
            }
        }
        assert!(parse_request(
            br#"{"type":"clipboard.copy","version":1,"window_id":1,"request_id":"copy-1"}"#
        )
        .is_ok());
        assert!(parse_request(br#"{"type":"clipboard.read","version":1}"#).is_err());
    }
}
