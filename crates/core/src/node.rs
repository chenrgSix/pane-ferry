//! Device and connection ownership shared by the management UI and publishers.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, io::Write, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RemoteContext {
    pub device_id: String,
    pub account: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub remote: RemoteContext,
    pub session_id: String,
}

/// A changed address is never evidence of device identity or pairing trust.
/// Late results must carry the exact generation which initiated the request.
#[derive(Default)]
pub struct Connections {
    generation: u64,
    remotes: HashMap<RemoteContext, u64>,
}
impl Connections {
    pub fn begin(&mut self, remote: RemoteContext) -> u64 {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("connection generation overflow");
        self.remotes.insert(remote, self.generation);
        self.generation
    }
    pub fn current(&self, remote: &RemoteContext, generation: u64) -> bool {
        self.remotes.get(remote) == Some(&generation)
    }
    pub fn disconnect(&mut self, remote: &RemoteContext) {
        self.remotes.remove(remote);
    }
}

pub use paneferry_platform::storage::private_directory;
pub fn device_id(directory: &Path) -> Result<String> {
    private_directory(directory)?;
    let lock_path = directory.join(".device-id.lock");
    if lock_path.is_symlink() {
        bail!("invalid identity lock")
    }
    let lock = paneferry_platform::storage::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.lock()?;
    let path = directory.join("device-id");
    // create_new avoids different identities when GUI and service start together.
    match paneferry_platform::storage::options()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(uuid::Uuid::new_v4().to_string().as_bytes())?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    paneferry_platform::storage::check_private_file(&path)?;
    let value = fs::read_to_string(path)?;
    if uuid::Uuid::parse_str(&value).is_err() {
        bail!("invalid device identity")
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_remote_session_numbers_and_late_results_remain_scoped() {
        let a = RemoteContext {
            device_id: "a".into(),
            account: "user".into(),
        };
        let b = RemoteContext {
            device_id: "b".into(),
            account: "user".into(),
        };
        let mut connections = Connections::default();
        let a1 = connections.begin(a.clone());
        let b1 = connections.begin(b.clone());
        let a2 = connections.begin(a.clone());
        assert!(!connections.current(&a, a1));
        assert!(connections.current(&a, a2));
        assert!(connections.current(&b, b1));
        assert_ne!(
            SessionKey {
                remote: a.clone(),
                session_id: "same".into()
            },
            SessionKey {
                remote: b.clone(),
                session_id: "same".into()
            }
        );
        connections.disconnect(&a);
        assert!(!connections.current(&a, a2));
        assert!(connections.current(&b, b1));
    }
}
