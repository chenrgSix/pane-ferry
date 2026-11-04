use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Installation {
    pub agent: PathBuf,
    pub network: PathBuf,
    pub receiver: PathBuf,
}

// Resolve all components from one installation. In particular a system package
// must never silently pick up an older helper from ~/.local/bin or PATH.
pub fn locate(executable: &Path, resources: Option<&Path>) -> Result<Installation, String> {
    let parent = executable.parent().ok_or("无法定位 Linux 安装目录。")?;
    let mut candidates = vec![(parent.to_path_buf(), parent.to_path_buf())];
    if parent.file_name().is_some_and(|name| name == "bin") {
        if let Some(prefix) = parent.parent() {
            candidates.push((prefix.join("share/paneferry"), parent.to_path_buf()));
        }
    }
    if let Some(resources) = resources {
        candidates.push((resources.join("linux-client"), resources.to_path_buf()));
        candidates.push((resources.to_path_buf(), resources.to_path_buf()));
    }
    for (client, binaries) in candidates {
        let installation = Installation {
            agent: binaries.join("paneferry-agent"),
            network: binaries.join("paneferry-network"),
            receiver: client.join("receiver_main.py"),
        };
        if [
            &installation.agent,
            &installation.network,
            &installation.receiver,
        ]
        .iter()
        .all(|path| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()))
        {
            return Ok(installation);
        }
    }
    Err("Linux 组件不完整，请重新安装窗渡。".into())
}

#[cfg(target_os = "linux")]
pub fn installation(app: &tauri::AppHandle) -> Result<Installation, String> {
    use tauri::Manager;
    locate(
        &std::env::current_exe().map_err(|_| "无法定位 Linux 启动器。")?,
        app.path().resource_dir().ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn files(client: &Path, binaries: &Path) {
        fs::create_dir_all(client).unwrap();
        fs::create_dir_all(binaries).unwrap();
        fs::write(client.join("receiver_main.py"), "# receiver").unwrap();
        for name in ["paneferry-agent", "paneferry-network"] {
            fs::write(binaries.join(name), "helper").unwrap();
        }
    }
    #[test]
    fn user_system_and_resource_layouts_keep_helpers_together() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path();
        let bin = prefix.join("bin");
        let client = prefix.join("share/paneferry");
        files(&client, &bin);
        let user = locate(&bin.join("paneferry-launcher"), None).unwrap();
        assert_eq!(user.agent, bin.join("paneferry-agent"));
        assert_eq!(user.receiver, client.join("receiver_main.py"));
        let system = prefix.join("lib/paneferry");
        files(&system, &system);
        assert_eq!(
            locate(&system.join("paneferry-launcher"), None)
                .unwrap()
                .network,
            system.join("paneferry-network")
        );
        let resources = prefix.join("resources");
        files(&resources.join("linux-client"), &resources);
        fs::remove_file(bin.join("paneferry-agent")).unwrap();
        assert!(locate(&bin.join("paneferry-launcher"), None).is_err());
        assert_eq!(
            locate(&bin.join("paneferry-launcher"), Some(&resources))
                .unwrap()
                .agent,
            resources.join("paneferry-agent")
        );
    }
}
