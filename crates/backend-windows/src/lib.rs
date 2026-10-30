//! Current interactive desktop and locally discovered applications. No service desktop or elevation.
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod input;
#[cfg(windows)]
mod native;
#[cfg(any(windows, test))]
mod pixels;
#[cfg(windows)]
pub use native::{CaptureRuntime, Desktop};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub description: String,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub handle: usize,
    pub pid: u32,
    pub started: u64,
    pub generation: usize,
    pub name: String,
    /// Preference identity only; never used to authorize a window attachment.
    pub application_key: String,
    pub auto_connectable: bool,
}

#[cfg(any(windows, test))]
fn application_key(executable: &str) -> String {
    use sha2::{Digest, Sha256};
    let path = executable.replace('/', "\\").to_lowercase();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    format!("exe-{:x}", Sha256::digest(path.as_bytes()))
}

#[cfg(test)]
mod application_tests {
    use super::application_key;

    #[test]
    fn preferences_follow_the_executable_not_its_spelling_or_basename() {
        let key = application_key(r"C:\Apps\Editor.exe");
        assert_eq!(key, application_key("c:/apps/editor.exe"));
        assert_eq!(key, application_key(r"\\?\C:\Apps\Editor.exe"));
        assert_ne!(key, application_key(r"D:\Other\Editor.exe"));
        assert_eq!(key.len(), 68);
        assert!(key.starts_with("exe-"));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Geometry {
    #[serde(default)]
    pub content_revision: u64,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
}

pub struct VideoFrame {
    pub content_revision: u64,
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
    fn applications(&mut self) -> Result<Vec<Application>> {
        Ok(Vec::new())
    }
    /// Resolve the local catalog again; false polls/reuses without launching a
    /// second process after an interrupted request with the same request ID.
    fn launch_application(&mut self, _id: &str, _start: bool) -> Result<Target> {
        anyhow::bail!("app_not_approved")
    }
    fn windows(&mut self) -> Result<Vec<Target>>;
    fn geometry(&mut self, target: &Target) -> Result<Geometry>;
    fn attach(&mut self, target: &Target, fps: u32) -> Result<()>;
    fn keyframe(&mut self, target: &Target) -> Result<()>;
    fn frame(&mut self, target: &Target) -> Result<Option<VideoFrame>>;
    fn input(&mut self, target: &Target, input: Input) -> Result<()>;
    fn detach(&mut self);
}
