use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
pub const MAX_LINE: usize = 16 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 8_000_000;
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
        backend: String,
    },
    #[serde(rename = "session.resume")]
    Resume { session_id: String },
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
    Ok(serde_json::from_value(v)?)
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
    pub geometry_revision: u64,
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
}
