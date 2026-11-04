//! Import the retired GTK manager's preferences once, without rewriting them.
use serde_json::{json, Value};
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

pub fn saved_connections(path: &Path) -> Option<Vec<Value>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 {
        return None;
    }
    let Value::Array(rows) = serde_json::from_slice::<Value>(&fs::read(path).ok()?).ok()? else {
        return None;
    };
    Some(
        rows.iter()
            .take(100)
            .filter_map(|row| {
                let id = row["peerId"].as_str()?;
                if uuid::Uuid::parse_str(id).ok()?.to_string() != id {
                    return None;
                }
                let name = row["name"].as_str()?;
                let address = row["address"].as_str()?;
                if name.is_empty() || name.chars().count() > 80 || address.len() > 128 {
                    return None;
                }
                // Only bookmarks cross the local UI boundary, never credentials or any
                // other keys which may have been added to an old JSON record.
                Some(json!({"peerId":id,"name":name,"address":address}))
            })
            .collect(),
    )
}

pub fn migrate(legacy: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(legacy) {
        Ok(value) if value.is_file() && value.len() <= 64 * 1024 => value,
        _ => return Ok(()),
    };
    let _ = metadata;
    let Ok(value) = serde_json::from_slice::<Value>(&fs::read(legacy)?) else {
        return Ok(());
    };
    if !value.is_object() {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    fn write_missing(directory: &Path, name: &str, value: Value) -> std::io::Result<()> {
        let target = directory.join(name);
        if target.try_exists()? {
            return Ok(());
        }
        let pending = directory.join(format!(".migration-{}", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&pending)?;
            file.write_all(&serde_json::to_vec(&value)?)?;
            file.sync_all()?;
            // Never overwrite a preference written concurrently by another GUI.
            match fs::hard_link(&pending, target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        })();
        let _ = fs::remove_file(pending);
        result
    }
    write_missing(
        destination,
        "display-picture-mode.json",
        json!(if value["source_size"] == true {
            "source"
        } else {
            "adapt"
        }),
    )?;
    write_missing(
        destination,
        "display-video-color.json",
        json!(if value["full_color"] == true {
            "444-atlas-v1"
        } else {
            "420"
        }),
    )?;
    fn peers(value: &Value) -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .take(32)
            .filter_map(|item| {
                let id = item.as_str()?.strip_prefix("quic:")?;
                uuid::Uuid::parse_str(id)
                    .ok()
                    .filter(|parsed| parsed.to_string() == id)
                    .map(|_| id.to_string())
            })
            .collect()
    }
    write_missing(
        destination,
        "launcher-defaults.json",
        json!({"applicationScene": value["application_scene"].as_bool().unwrap_or(false)}),
    )?;
    write_missing(
        destination,
        "device-audio.json",
        json!(peers(&value["audio"])),
    )?;
    write_missing(
        destination,
        "automatic-windows.json",
        json!({"rules":[],"devices":peers(&value["takeover"]).iter()
        .map(|id| json!({"peerId":id,"enabled":true,"videoFps":60})).collect::<Vec<_>>() }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_bookmarks_independently_and_omits_unknown_fields() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("saved-connections-v1.json");
        let id = uuid::Uuid::new_v4().to_string();
        fs::write(&file, serde_json::to_vec(&json!([{"peerId":id,"name":"Desktop","address":"192.168.8.20:42424","invitation":"not-a-bookmark"},{"peerId":"host"}])).unwrap()).unwrap();
        assert_eq!(
            saved_connections(&file).unwrap(),
            vec![json!({"peerId":id,"name":"Desktop","address":"192.168.8.20:42424"})]
        );
        assert!(!directory.path().join("linux-preferences.json").exists());
        fs::write(&file, b"{invalid").unwrap();
        assert!(saved_connections(&file).is_none());
    }
    #[test]
    fn legacy_import_preserves_existing_settings_and_original_file() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = directory.path().join("linux-preferences.json");
        let target = directory.path().join("launcher");
        let id = uuid::Uuid::new_v4().to_string();
        let old = serde_json::to_vec(&json!({"source_size":true,"full_color":true,"audio":[format!("quic:{id}"),"bad"],"takeover":[format!("quic:{id}")],"render_scale":2})).unwrap();
        fs::write(&legacy, &old).unwrap();
        migrate(&legacy, &target).unwrap();
        assert_eq!(fs::read(&legacy).unwrap(), old);
        let audio: Vec<String> =
            serde_json::from_slice(&fs::read(target.join("device-audio.json")).unwrap()).unwrap();
        assert_eq!(audio, vec![id]);
        fs::write(target.join("display-picture-mode.json"), b"\"adapt\"").unwrap();
        migrate(&legacy, &target).unwrap();
        assert_eq!(
            fs::read(target.join("display-picture-mode.json")).unwrap(),
            b"\"adapt\""
        );
    }
}
