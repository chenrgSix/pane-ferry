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
pub struct Service {
    path: PathBuf,
    mode: Mutex<PictureMode>,
}
impl Service {
    pub fn open(path: PathBuf) -> Self {
        let mode = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            mode: Mutex::new(mode),
        }
    }
    pub fn mode(&self) -> PictureMode {
        *self.mode.lock().unwrap()
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
