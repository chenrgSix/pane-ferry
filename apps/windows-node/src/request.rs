use anyhow::{ensure, Result};
use paneferry_backend_windows::{input, Input};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "node.info")]
    Info,
    #[serde(rename = "sharing.set")]
    Sharing { enabled: bool },
    #[serde(rename = "app.list")]
    Apps,
    #[serde(rename = "window.list")]
    Windows,
    #[serde(rename = "desktop.list")]
    Desktops,
    #[serde(rename = "session.list")]
    Sessions,
    #[serde(rename = "video.enable")]
    Video {
        codec: String,
        #[serde(default = "fps")]
        max_fps: u32,
        #[serde(default)]
        feedback: bool,
    },
    #[serde(rename = "video.keyframe")]
    KeyFrame { window_id: u32 },
    #[serde(rename = "app.launch")]
    Launch {
        app_id: String,
        request_id: String,
        backend: String,
        #[serde(default = "yes")]
        attach: bool,
        #[serde(default = "scale")]
        scale_factor: u8,
    },
    #[serde(rename = "session.resume")]
    Resume { session_id: String },
    #[serde(rename = "session.detach")]
    Detach,
    #[serde(rename = "window.focus")]
    Focus { window_id: u32 },
    #[serde(rename = "window.close.request")]
    Close { window_id: u32 },
    #[serde(rename = "window.configure")]
    Resize {
        window_id: u32,
        width: u32,
        height: u32,
    },
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
    #[serde(rename = "input.key.logical")]
    Logical {
        window_id: u32,
        key: String,
        pressed: bool,
    },
    #[serde(rename = "input.text.commit")]
    Text { window_id: u32, text: String },
}
fn fps() -> u32 {
    30
}
fn yes() -> bool {
    true
}
fn scale() -> u8 {
    1
}
pub fn parse(mut value: Value) -> Result<Request> {
    ensure!(value["version"] == 1, "invalid_request");
    value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("invalid_request"))?
        .remove("version");
    serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid_request"))
}
impl Request {
    pub fn input(self) -> Result<(u32, Input)> {
        Ok(match self {
            Self::Focus { window_id } => (window_id, Input::Focus),
            Self::Close { window_id } => (window_id, Input::Close),
            Self::Resize {
                window_id,
                width,
                height,
            } => {
                paneferry_protocol::dimensions(width, height)
                    .map_err(|_| anyhow::anyhow!("invalid_request"))?;
                (window_id, Input::Resize { width, height })
            }
            Self::Pointer {
                window_id,
                x,
                y,
                button,
                pressed,
            } => {
                ensure!(button <= 7, "invalid_request");
                (
                    window_id,
                    Input::Pointer {
                        x,
                        y,
                        button,
                        pressed,
                    },
                )
            }
            Self::Key {
                window_id,
                keysym,
                pressed,
            } => (
                window_id,
                Input::Key {
                    virtual_key: input::keysym(keysym)
                        .ok_or_else(|| anyhow::anyhow!("input_unsupported"))?,
                    pressed,
                },
            ),
            Self::Logical {
                window_id,
                key,
                pressed,
            } => (
                window_id,
                Input::Key {
                    virtual_key: input::logical(&key)
                        .ok_or_else(|| anyhow::anyhow!("input_unsupported"))?,
                    pressed,
                },
            ),
            Self::Text { window_id, text } => {
                paneferry_protocol::validate_text(&text)
                    .map_err(|_| anyhow::anyhow!("invalid_request"))?;
                (window_id, Input::Text(text))
            }
            _ => anyhow::bail!("invalid_request"),
        })
    }
}
