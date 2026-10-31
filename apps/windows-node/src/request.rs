use anyhow::{ensure, Result};
use paneferry_backend_windows::{input, Input};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "audio.subscribe")]
    Audio,
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
    #[serde(rename = "desktop.start")]
    StartDesktop {
        monitor_id: String,
        request_id: String,
    },
    #[serde(rename = "session.list")]
    Sessions,
    #[serde(rename = "video.enable")]
    Video {
        codec: String,
        #[serde(default)]
        color: paneferry_backend_windows::color::ColorMode,
        #[serde(default = "fps")]
        max_fps: u32,
        #[serde(default)]
        feedback: bool,
    },
    #[serde(rename = "video.configure")]
    VideoConfigure { max_fps: u32 },
    #[serde(rename = "video.feedback")]
    VideoFeedback {
        frame_id: u64,
        #[serde(default)]
        timing: Option<crate::video_flow::FeedbackTiming>,
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
    let request = serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid_request"))?;
    if let Request::VideoFeedback {
        timing: Some(timing),
        ..
    } = &request
    {
        ensure!(timing.valid(), "invalid_request");
    }
    Ok(request)
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

#[cfg(test)]
mod tests {
    use super::*;
    use paneferry_backend_windows::color::ColorMode;
    #[test]
    fn feedback_timing_is_optional_bounded_and_strict() {
        let legacy = serde_json::json!({"version":1,"type":"video.feedback","frame_id":7});
        assert!(parse(legacy.clone()).is_ok());
        let mut timed = legacy;
        timed["timing"] = serde_json::json!({"queue_us":1000,"decode_us":2000,"present_us":3000,"feedback_wait_us":4000});
        assert!(parse(timed.clone()).is_ok());
        for invalid in [
            serde_json::json!(-1),
            serde_json::json!(5_000_001),
            serde_json::json!(true),
            serde_json::json!(1.5),
        ] {
            let mut bad = timed.clone();
            bad["timing"]["decode_us"] = invalid;
            assert!(parse(bad).is_err());
        }
        timed["timing"]["unexpected"] = serde_json::json!(1);
        assert!(parse(timed).is_err());
    }
    #[test]
    fn color_requires_explicit_supported_negotiation() {
        for (color, expected) in [
            (None, ColorMode::Compatible),
            (Some("420"), ColorMode::Compatible),
            (Some("444-atlas-v1"), ColorMode::Full),
            (Some("444-atlas-v2"), ColorMode::FullAligned),
        ] {
            let mut value = serde_json::json!({"type":"video.enable","codec":"h264"});
            if let Some(color) = color {
                value["color"] = color.into();
            }
            assert!(
                matches!(serde_json::from_value::<Request>(value).unwrap(),Request::Video {color,..} if color==expected)
            );
        }
        for color in [
            serde_json::json!("444"),
            serde_json::json!(true),
            serde_json::json!(null),
        ] {
            assert!(serde_json::from_value::<Request>(
                serde_json::json!({"type":"video.enable","codec":"h264","color":color})
            )
            .is_err());
        }
    }
}
