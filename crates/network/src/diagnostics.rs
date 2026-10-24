//! Small, private connection logs survive launcher stderr redirection. Never
//! record requests, certificates, pairing tokens, clipboard data or frames.
use paneferry_platform::storage;
use serde_json::json;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const LIMIT: u64 = 512 * 1024;
pub(crate) struct Diagnostics {
    path: PathBuf,
    file: Mutex<Option<File>>,
}
fn open(path: &Path) -> io::Result<File> {
    match fs::symlink_metadata(path) {
        Ok(_) => storage::check_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    storage::options().append(true).create(true).open(path)
}
impl Diagnostics {
    pub(crate) fn new(directory: &Path) -> Self {
        Self {
            path: directory.join("network.log"),
            file: Mutex::new(None),
        }
    }
    pub(crate) fn record(&self, event: &'static str, peer: Option<&str>, detail: Option<&str>) {
        let value = json!({
            "time_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "pid":std::process::id(), "event":event, "peer":peer,
            "detail":detail.map(|s|s.chars().take(512).collect::<String>())
        });
        if let Err(error) = self.write(value.to_string().as_bytes()) {
            eprintln!("paneferry-network: cannot write connection diagnostics: {error}");
        }
    }
    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.file.lock().unwrap();
        if file.is_none() {
            *file = Some(open(&self.path)?);
        }
        if file.as_ref().unwrap().metadata()?.len() + bytes.len() as u64 + 1 > LIMIT {
            // Close first for Windows. Keep at most one previous generation.
            file.take();
            storage::replace(&self.path, &self.path.with_extension("log.1"))?;
            *file = Some(open(&self.path)?);
        }
        let file = file.as_mut().unwrap();
        file.write_all(bytes)?;
        file.write_all(b"\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_are_private_bounded_and_keep_the_latest_failure() {
        let root = tempfile::tempdir().unwrap();
        storage::private_directory(root.path()).unwrap();
        let log = Diagnostics::new(root.path());
        let detail = "test detail ".repeat(100);
        for _ in 0..1000 {
            log.record("route_failed", Some("test-peer"), Some(&detail));
        }
        log.record("connection_closed", Some("test-peer"), Some("timed out"));
        for path in [&log.path, &log.path.with_extension("log.1")] {
            storage::check_private_file(path).unwrap();
            assert!(fs::metadata(path).unwrap().len() <= LIMIT);
        }
        let data = fs::read_to_string(&log.path).unwrap();
        let last: serde_json::Value = serde_json::from_str(data.lines().last().unwrap()).unwrap();
        assert_eq!(last["event"], "connection_closed");
        assert_eq!(last["detail"], "timed out");
        assert_eq!(last["pid"], std::process::id());
    }
}
