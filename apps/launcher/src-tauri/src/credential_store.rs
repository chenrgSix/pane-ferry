use remoteapp_transport::Endpoint;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

const MAX_BYTES: u64 = 1024 * 1024;
pub const FILE_NAME: &str = "ssh-connections.json";

#[derive(Deserialize, Serialize)]
struct Connection {
    #[serde(flatten)]
    endpoint: Endpoint,
    password: String,
}
#[derive(Deserialize, Serialize)]
struct Configuration {
    version: u8,
    connections: Vec<Connection>,
}

/// Only the Mac backend writes connection secrets. Browser preferences retain
/// non-secret UI state; SSH and WindowHost read this same private configuration.
pub fn save(directory: &Path, endpoint: Endpoint, password: String) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    endpoint.validate().map_err(|error| error.to_string())?;
    if password.is_empty() || password.len() > 4096 || password.contains(['\0', '\n', '\r']) {
        return Err("密码格式无效。".into());
    }
    fs::create_dir_all(directory).map_err(|_| "无法创建连接配置目录。")?;
    if fs::symlink_metadata(directory)
        .map_err(|_| "无法检查连接配置目录。")?
        .file_type()
        .is_symlink()
    {
        return Err("连接配置目录不能是符号链接。".into());
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法保护连接配置目录。")?;
    let lock_path = directory.join(".ssh-connections.lock");
    if lock_path.is_symlink() {
        return Err("连接配置锁无效。".into());
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)
        .map_err(|_| "无法打开连接配置锁。")?;
    lock.lock().map_err(|_| "无法锁定连接配置。")?;
    let path = directory.join(FILE_NAME);
    let mut configuration = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES {
                return Err("已有连接配置无效，未覆盖。".into());
            }
            let data = fs::read(&path).map_err(|_| "无法读取连接配置。")?;
            let value: Configuration =
                serde_json::from_slice(&data).map_err(|_| "已有连接配置损坏，未覆盖。")?;
            if value.version != 1 || value.connections.len() > 128 {
                return Err("连接配置版本或数量不支持。".into());
            }
            value
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Configuration {
            version: 1,
            connections: vec![],
        },
        Err(_) => return Err("无法检查已有连接配置。".into()),
    };
    configuration.connections.retain(|connection| {
        connection.endpoint.host != endpoint.host
            || connection.endpoint.user != endpoint.user
            || connection.endpoint.port != endpoint.port
    });
    configuration
        .connections
        .push(Connection { endpoint, password });
    if configuration.connections.len() > 128 {
        return Err("保存的连接已达上限。".into());
    }
    let data = serde_json::to_vec(&configuration).map_err(|_| "无法编码连接配置。")?;
    if data.len() as u64 > MAX_BYTES {
        return Err("连接配置超过大小上限。".into());
    }
    // A lock serializes writers in separate launcher processes. Atomic replace
    // lets running WindowHosts read either complete version without that lock.
    let temporary = directory.join(format!(".ssh-connections-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| "无法创建临时连接配置。")?;
        file.write_all(&data).map_err(|_| "无法写入连接配置。")?;
        file.sync_all().map_err(|_| "无法保存连接配置。")?;
        fs::rename(&temporary, &path).map_err(|_| "无法更新连接配置。")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn endpoint(port: u16) -> Endpoint {
        Endpoint {
            host: "fixture.invalid".into(),
            user: "fixture".into(),
            port,
        }
    }
    fn directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "remoteapp-service-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn persistent_profiles_are_private_endpoint_scoped_and_updated_without_duplicates() {
        let root = directory();
        save(&root, endpoint(22), "first fixture".into()).unwrap();
        save(&root, endpoint(2222), "other fixture".into()).unwrap();
        save(&root, endpoint(22), "new fixture".into()).unwrap();
        let stored: Configuration =
            serde_json::from_slice(&fs::read(root.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(stored.connections.len(), 2);
        assert_eq!(
            stored
                .connections
                .iter()
                .find(|value| value.endpoint.port == 22)
                .unwrap()
                .password,
            "new fixture"
        );
        assert_eq!(
            stored
                .connections
                .iter()
                .find(|value| value.endpoint.port == 2222)
                .unwrap()
                .password,
            "other fixture"
        );
        assert_eq!(
            fs::metadata(root.join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rejected_or_damaged_profiles_never_replace_existing_data_or_leak_secrets() {
        let root = directory();
        save(&root, endpoint(22), "fixture".into()).unwrap();
        let path = root.join(FILE_NAME);
        let before = fs::read(&path).unwrap();
        let error = save(&root, endpoint(22), "private\nvalue".into()).unwrap_err();
        assert!(!error.contains("private"));
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::write(&path, b"damaged fixture").unwrap();
        assert!(save(&root, endpoint(22), "replacement".into()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"damaged fixture");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.join("unrelated"), &path).unwrap();
        assert!(save(&root, endpoint(22), "replacement".into()).is_err());
        assert!(!root.join("unrelated").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn simultaneous_backend_updates_preserve_other_connections() {
        let root = directory();
        let threads = (2200..2208)
            .map(|port| {
                let directory = root.clone();
                std::thread::spawn(move || {
                    save(&directory, endpoint(port), "parallel fixture".into()).unwrap()
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let stored: Configuration =
            serde_json::from_slice(&fs::read(root.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(stored.connections.len(), 8);
        fs::remove_dir_all(root).unwrap();
    }
}
