use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const EXECUTABLE: &str = "Contents/MacOS/PaneFerryWindowHost";
const FALLBACK_ICON: &str = "Contents/Resources/PaneFerry.png";

fn display_name(value: Option<&str>) -> String {
    let value: String = value
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect();
    let value = value.trim();
    if value.is_empty() {
        "Linux 应用".into()
    } else {
        value.into()
    }
}

fn filename(name: &str) -> String {
    let mut value = String::new();
    for character in name.chars() {
        let character = if matches!(character, '/' | ':' | '\\') {
            '_'
        } else {
            character
        };
        if value.len() + character.len_utf8() > 180 {
            break;
        }
        value.push(character);
    }
    let value = value.trim_matches([' ', '.']);
    format!(
        "{}.app",
        if value.is_empty() {
            "Linux 应用"
        } else {
            value
        }
    )
}

fn regular_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    fs::read(path)
}

fn private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn write_new(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

// A one-day grace period covers prepare→spawn and older hosts without leases.
// Running executable paths pin their entire immutable generation.
fn collect_generations(
    directory: &Path,
    current: &Path,
    running: &[PathBuf],
    now: std::time::SystemTime,
) -> std::io::Result<()> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() != 64
            || !name.bytes().all(|b| b.is_ascii_hexdigit())
            || !entry.file_type()?.is_dir()
        {
            continue;
        }
        let path = entry.path();
        if path == current || running.iter().any(|binary| binary.starts_with(&path)) {
            continue;
        }
        entries.push((path, entry.metadata()?.modified()?));
    }
    entries.sort_by_key(|(_, modified)| *modified);
    let mut count = entries.len();
    for (path, modified) in entries {
        let age = now.duration_since(modified).unwrap_or_default();
        if age.as_secs() >= 86400 && (age.as_secs() > 7 * 86400 || count > 32) {
            fs::remove_dir_all(path)?;
            count -= 1;
        }
    }
    Ok(())
}
fn collect_idle_generations(directory: &Path, current: &Path) {
    // Failure to inspect running processes means no deletion, never guessing.
    if let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-axo", "comm="])
        .output()
    {
        if output.status.success() {
            if let Ok(paths) = String::from_utf8(output.stdout) {
                let running: Vec<_> = paths.lines().map(|p| PathBuf::from(p.trim())).collect();
                let _ =
                    collect_generations(directory, current, &running, std::time::SystemTime::now());
            }
        }
    }
}

/// Dock and application switching read bundle identity before AppKit starts.
/// Give each remote application its own immutable local bundle, without editing
/// the installed package or rewriting a running host during an application update.
pub fn prepare(
    directory: &Path,
    source: &Path,
    endpoint: &impl serde::Serialize,
    app_id: &str,
    name: Option<&str>,
) -> Result<PathBuf, String> {
    prepare_inner(directory, source, endpoint, app_id, name)
        .map_err(|_| "无法准备本机应用窗口入口，请重试连接。".into())
}

fn prepare_inner(
    directory: &Path,
    source: &Path,
    endpoint: &impl serde::Serialize,
    app_id: &str,
    name: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let name = display_name(name);
    let binary = regular_bytes(source)?;
    let source_bundle = source
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("invalid WindowHost bundle")?;
    let icon = regular_bytes(&source_bundle.join(FALLBACK_ICON))?;
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&(1, endpoint, app_id, &name))?);
    digest.update(Sha256::digest(&binary));
    digest.update(Sha256::digest(&icon));
    let identity = format!("{:x}", digest.finalize());
    let mut properties = plist::Dictionary::new();
    for (key, value) in [
        ("CFBundleName", name.as_str()),
        ("CFBundleDisplayName", name.as_str()),
        (
            "CFBundleIdentifier",
            &format!("io.lanremoteapp.host.{identity}"),
        ),
        ("CFBundleExecutable", "PaneFerryWindowHost"),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleInfoDictionaryVersion", "6.0"),
        ("CFBundleVersion", "1"),
        ("CFBundleShortVersionString", "0.1.0"),
        ("LSMinimumSystemVersion", "12.0"),
    ] {
        properties.insert(key.into(), value.into());
    }
    properties.insert("NSHighResolutionCapable".into(), true.into());
    let mut info = Vec::new();
    plist::Value::Dictionary(properties).to_writer_xml(&mut info)?;

    private_directory(directory)?;
    let lock_path = directory.join(".bundles.lock");
    if lock_path.is_symlink() {
        return Err("invalid bundle lock".into());
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)?;
    lock.lock()?;
    let generation = directory.join(&identity);
    let bundle = generation.join(filename(&name));
    if generation.try_exists()? {
        for path in [
            &generation,
            &bundle,
            &bundle.join("Contents"),
            &bundle.join("Contents/MacOS"),
            &bundle.join("Contents/Resources"),
        ] {
            if !fs::symlink_metadata(path)?.file_type().is_dir() {
                return Err("invalid cached bundle".into());
            }
        }
        if regular_bytes(&bundle.join(EXECUTABLE))? != binary
            || regular_bytes(&bundle.join(FALLBACK_ICON))? != icon
            || regular_bytes(&bundle.join("Contents/Info.plist"))? != info
        {
            return Err("invalid cached bundle contents".into());
        }
        fs::File::open(&generation)?.set_modified(std::time::SystemTime::now())?;
        collect_idle_generations(directory, &generation);
        return Ok(bundle.join(EXECUTABLE));
    }
    // The lock serializes both threads and separate launchers. Publish only a
    // complete generation; existing generations and running binaries stay intact.
    let staging = directory.join(format!(
        ".building-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir(&staging)?;
    let result = (|| {
        let staged = staging.join(filename(&name));
        private_directory(&staged.join("Contents/MacOS"))?;
        private_directory(&staged.join("Contents/Resources"))?;
        write_new(&staged.join(EXECUTABLE), &binary, 0o700)?;
        write_new(&staged.join(FALLBACK_ICON), &icon, 0o600)?;
        write_new(&staged.join("Contents/Info.plist"), &info, 0o600)?;
        fs::rename(&staging, &generation)?;
        collect_idle_generations(directory, &generation);
        Ok(bundle.join(EXECUTABLE))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_collection_preserves_current_running_and_recent_generations() {
        let root = std::env::temp_dir().join(format!("paneferry-gc-{}", std::process::id()));
        private_directory(&root).unwrap();
        let paths: Vec<_> = (1..=4).map(|n| root.join(format!("{n:064x}"))).collect();
        for path in &paths {
            private_directory(path).unwrap();
        }
        let now = std::time::SystemTime::now();
        for path in &paths[..3] {
            fs::File::open(path)
                .unwrap()
                .set_modified(now - std::time::Duration::from_secs(8 * 86400))
                .unwrap();
        }
        collect_generations(&root, &paths[0], &[paths[1].join(EXECUTABLE)], now).unwrap();
        assert!(paths[0].exists() && paths[1].exists() && !paths[2].exists() && paths[3].exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn names_are_bundle_metadata_and_safe_paths_with_isolated_immutable_generations() {
        let root = std::env::temp_dir().join(format!("paneferry-bundle-{}", std::process::id()));
        let source = root.join("source.app");
        private_directory(&source.join("Contents/MacOS")).unwrap();
        private_directory(&source.join("Contents/Resources")).unwrap();
        fs::write(source.join(EXECUTABLE), b"fixture binary v1").unwrap();
        fs::write(source.join(FALLBACK_ICON), b"fixture icon").unwrap();
        let endpoint = serde_json::json!({"transport":"quic","device_id":"device-a"});
        let cache = root.join("cache");
        let original = prepare(
            &cache,
            &source.join(EXECUTABLE),
            &endpoint,
            "app1",
            Some("../设置 & <测试>🙂"),
        )
        .unwrap();
        let bundle = original
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let info = plist::Value::from_file(bundle.join("Contents/Info.plist")).unwrap();
        assert_eq!(
            info.as_dictionary().unwrap()["CFBundleDisplayName"].as_string(),
            Some("../设置 & <测试>🙂")
        );
        assert_eq!(bundle.file_name().unwrap(), "_设置 & <测试>🙂.app");
        assert_eq!(
            prepare(
                &cache,
                &source.join(EXECUTABLE),
                &endpoint,
                "app1",
                Some("../设置 & <测试>🙂")
            )
            .unwrap(),
            original
        );
        assert_eq!(
            fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&original).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_ne!(
            prepare(
                &cache,
                &source.join(EXECUTABLE),
                &endpoint,
                "app2",
                Some("../设置 & <测试>🙂")
            )
            .unwrap(),
            original
        );
        let other_endpoint = serde_json::json!({"transport":"quic","device_id":"device-b"});
        assert_ne!(
            prepare(
                &cache,
                &source.join(EXECUTABLE),
                &other_endpoint,
                "app1",
                Some("../设置 & <测试>🙂")
            )
            .unwrap(),
            original
        );
        assert_ne!(
            prepare(
                &cache,
                &source.join(EXECUTABLE),
                &endpoint,
                "app1",
                Some("Firefox")
            )
            .unwrap(),
            original
        );
        fs::write(source.join(EXECUTABLE), b"fixture binary v2").unwrap();
        assert_ne!(
            prepare(
                &cache,
                &source.join(EXECUTABLE),
                &endpoint,
                "app1",
                Some("../设置 & <测试>🙂")
            )
            .unwrap(),
            original
        );
        assert_eq!(
            fs::read(&original).unwrap(),
            b"fixture binary v1",
            "updates never overwrite a running generation"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_names_are_usable_and_path_components_cannot_escape() {
        assert_eq!(display_name(None), "Linux 应用");
        assert_eq!(display_name(Some(" \n\0\t ")), "Linux 应用");
        assert_eq!(display_name(Some(" 设置\n")), "设置");
        assert_eq!(filename("../../"), "_.._.app");
        assert_eq!(
            Path::new(&filename(&"😀".repeat(100))).components().count(),
            1
        );
        assert!(filename(&"😀".repeat(100)).len() <= 184);
    }

    #[test]
    fn cached_bundles_reject_symlink_retargeting_without_writing_outside() {
        let root =
            std::env::temp_dir().join(format!("paneferry-bundle-links-{}", std::process::id()));
        let source = root.join("source.app");
        private_directory(&source.join("Contents/MacOS")).unwrap();
        private_directory(&source.join("Contents/Resources")).unwrap();
        fs::write(source.join(EXECUTABLE), b"fixture binary").unwrap();
        fs::write(source.join(FALLBACK_ICON), b"fixture icon").unwrap();
        let endpoint = serde_json::json!({"transport":"quic","device_id":"device-a"});
        let cache = root.join("cache");
        let host = prepare(
            &cache,
            &source.join(EXECUTABLE),
            &endpoint,
            "app",
            Some("设置"),
        )
        .unwrap();
        let unrelated = root.join("unrelated");
        fs::write(&unrelated, b"unrelated content").unwrap();
        fs::remove_file(&host).unwrap();
        std::os::unix::fs::symlink(&unrelated, &host).unwrap();
        assert!(prepare(
            &cache,
            &source.join(EXECUTABLE),
            &endpoint,
            "app",
            Some("设置")
        )
        .is_err());
        assert_eq!(fs::read(&unrelated).unwrap(), b"unrelated content");
        let linked_cache = root.join("linked-cache");
        std::os::unix::fs::symlink(&cache, &linked_cache).unwrap();
        assert!(prepare(
            &linked_cache,
            &source.join(EXECUTABLE),
            &endpoint,
            "app",
            Some("Firefox")
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
