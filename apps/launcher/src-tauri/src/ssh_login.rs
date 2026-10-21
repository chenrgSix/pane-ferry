use remoteapp_transport::Endpoint;
use std::path::{Path, PathBuf};

const HELPER: &str = include_str!("../../../../scripts/connect-ssh.sh");

fn script(endpoint: &Endpoint) -> Result<String, String> {
    endpoint.validate().map_err(|error| error.to_string())?;
    // The endpoint grammar excludes quotes, substitutions and control bytes.
    // Terminal receives a document path, never an interpolated AppleScript.
    Ok(format!(
        "#!/bin/bash\nset -- '{}' '{}' '{}'\n{}",
        endpoint.host,
        endpoint.user,
        endpoint.port,
        HELPER
            .strip_prefix("#!/usr/bin/env bash\n")
            .ok_or("登录脚本格式无效。")?
    ))
}

#[cfg(target_os = "macos")]
fn write_script(cache: &Path, endpoint: &Endpoint) -> Result<PathBuf, String> {
    use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, os::unix::fs::PermissionsExt};
    let content = script(endpoint)?;
    let directory = cache.join("ssh-login");
    fs::create_dir_all(&directory).map_err(|_| "无法创建本机 SSH 登录目录。")?;
    if fs::symlink_metadata(&directory)
        .map_err(|_| "无法检查登录目录。")?
        .file_type()
        .is_symlink()
    {
        return Err("SSH 登录目录不能是符号链接。".into());
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法保护登录目录。")?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "本机时间无效。")?
        .as_nanos();
    let path = directory.join(format!(
        "remoteapp-ssh-login-{}-{nonce}.command",
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .map_err(|_| "无法创建 SSH 登录脚本。")?;
    file.write_all(content.as_bytes())
        .map_err(|_| "无法写入 SSH 登录脚本。")?;
    Ok(path)
}

pub fn open(cache: &Path, endpoint: &Endpoint) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let path = write_script(cache, endpoint)?;
        let status = std::process::Command::new("/usr/bin/open")
            .args(["-a", "/System/Applications/Utilities/Terminal.app"])
            .arg(path)
            .status()
            .map_err(|_| "无法打开系统终端。")?;
        if status.success() {
            Ok(())
        } else {
            Err("无法打开 SSH 登录终端。".into())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cache, endpoint);
        Err("SSH 登录终端目前仅支持 macOS。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint() -> Endpoint {
        Endpoint {
            host: "192.168.1.20".into(),
            user: "alice".into(),
            port: 2222,
        }
    }
    #[test]
    fn terminal_document_uses_validated_endpoint_and_bundled_helper() {
        let body = script(&endpoint()).unwrap();
        assert!(body.starts_with("#!/bin/bash\nset -- '192.168.1.20' 'alice' '2222'\n"));
        assert!(body.contains("-oStrictHostKeyChecking=yes"));
        assert!(body.contains("-oControlMaster=auto"));
        for host in [
            "host;touch /tmp/unsafe",
            "$(id)",
            "host\ncommand",
            "-oProxyCommand=bad",
        ] {
            let mut target = endpoint();
            target.host = host.into();
            assert!(script(&target).is_err());
        }
        let mut target = endpoint();
        target.user = "alice' ; id".into();
        assert!(script(&target).is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn generated_login_documents_are_private_and_do_not_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "remoteapp-ssh-login-test-{}-{nonce}",
            std::process::id()
        ));
        let first = write_script(&root, &endpoint()).unwrap();
        let second = write_script(&root, &endpoint()).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            std::fs::metadata(first.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::read_to_string(first).unwrap(),
            script(&endpoint()).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
