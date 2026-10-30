//! Shared by manual launches, resumed sessions and automatic takeover.
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex};
use tauri::Manager;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PictureMode {
    #[default]
    Adapt,
    Source,
}
impl PictureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Adapt => "adapt",
            Self::Source => "source",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum VideoColor {
    #[default]
    #[serde(rename = "420")]
    Compatible,
    #[serde(rename = "444-atlas-v1")]
    Full,
}
impl VideoColor {
    #[cfg(not(windows))]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "420",
            Self::Full => "444-atlas-v1",
        }
    }
}
pub struct Service {
    path: PathBuf,
    mode: Mutex<PictureMode>,
    color: Mutex<VideoColor>,
}
impl Service {
    pub fn open(path: PathBuf) -> Self {
        let mode = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let color = fs::read(path.with_file_name("display-video-color.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            color: Mutex::new(color),
            path,
            mode: Mutex::new(mode),
        }
    }
    pub fn mode(&self) -> PictureMode {
        *self.mode.lock().unwrap()
    }
    pub fn color(&self) -> VideoColor {
        *self.color.lock().unwrap()
    }
    fn set_color(&self, mode: VideoColor) -> Result<(), String> {
        let mut current = self.color.lock().unwrap();
        let path = self.path.with_file_name("display-video-color.json");
        let save = || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let pending = path.with_extension("tmp");
            fs::write(&pending, serde_json::to_vec(&mode)?)?;
            fs::rename(pending, &path)
        };
        save().map_err(|_| "?????????????".to_string())?;
        *current = mode;
        Ok(())
    }
    fn set(&self, mode: PictureMode) -> Result<(), String> {
        let mut current = self.mode.lock().unwrap();
        let save = || -> std::io::Result<()> {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }
            let pending = self.path.with_extension("tmp");
            fs::write(&pending, serde_json::to_vec(&mode)?)?;
            fs::rename(pending, &self.path)
        };
        save().map_err(|_| "画面尺寸设置保存失败，请重试。".to_string())?;
        *current = mode;
        Ok(())
    }
}
#[tauri::command]
pub fn picture_mode(app: tauri::AppHandle) -> PictureMode {
    app.state::<Service>().mode()
}
#[tauri::command]
pub fn set_picture_mode(app: tauri::AppHandle, mode: PictureMode) -> Result<(), String> {
    app.state::<Service>().set(mode)
}

#[tauri::command]
pub fn video_color(app: tauri::AppHandle) -> VideoColor {
    app.state::<Service>().color()
}
#[tauri::command]
pub fn set_video_color(app: tauri::AppHandle, mode: VideoColor) -> Result<(), String> {
    app.state::<Service>().set_color(mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_persists_both_modes() {
        let directory = std::env::temp_dir().join(format!("pf-display-{}", uuid::Uuid::new_v4()));
        let path = directory.join("display.json");
        let service = Service::open(path.clone());
        assert_eq!(service.mode(), PictureMode::Adapt);
        for mode in [PictureMode::Source, PictureMode::Adapt] {
            service.set(mode).unwrap();
            assert_eq!(Service::open(path.clone()).mode(), mode);
        }
        assert_eq!(service.color(), VideoColor::Compatible);
        for color in [VideoColor::Full, VideoColor::Compatible] {
            service.set_color(color).unwrap();
            assert_eq!(Service::open(path.clone()).color(), color);
            assert_eq!(Service::open(path.clone()).mode(), PictureMode::Adapt);
        }
        fs::write(&path, b"\"unsupported\"").unwrap();
        assert_eq!(Service::open(path).mode(), PictureMode::Adapt);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn failed_save_does_not_change_active_mode() {
        let directory = std::env::temp_dir().join(format!("pf-display-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let service = Service::open(directory.clone());
        assert!(service.set(PictureMode::Source).is_err());
        assert_eq!(service.mode(), PictureMode::Adapt);
        let _ = fs::remove_file(directory.with_extension("tmp"));
        fs::remove_dir_all(directory).unwrap();
    }
}
