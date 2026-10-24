//! Current interactive desktop only. No process launch, service desktop or elevation.
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod input;
#[cfg(windows)]
mod native;
#[cfg(any(windows, test))]
mod pixels;
#[cfg(windows)]
pub use native::Desktop;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub handle: usize,
    pub pid: u32,
    pub started: u64,
    pub generation: usize,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Geometry {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
}

pub struct VideoFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stream_id: u64,
    pub key_frame: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Pointer {
        x: i32,
        y: i32,
        button: u8,
        pressed: bool,
    },
    Key {
        virtual_key: u16,
        pressed: bool,
    },
    Text(String),
    Focus,
    Resize {
        width: u32,
        height: u32,
    },
    Close,
}

/// All calls for an attached window stay on its owning worker thread.
pub trait WindowSystem {
    fn windows(&mut self) -> Result<Vec<Target>>;
    fn geometry(&mut self, target: &Target) -> Result<Geometry>;
    fn attach(&mut self, target: &Target, fps: u32) -> Result<()>;
    fn keyframe(&mut self, target: &Target) -> Result<()>;
    fn frame(&mut self, target: &Target) -> Result<Option<VideoFrame>>;
    fn input(&mut self, target: &Target, input: Input) -> Result<()>;
    fn detach(&mut self);
}
