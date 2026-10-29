//! Retire this application's old password profile; never touch system SSH state.
use std::{fs, io, path::Path};

pub fn retire(directory: &Path) -> io::Result<()> {
    // Unlink the exact application-owned filename, without reading credentials
    // or following a symlink. Existing QUIC identity and pairings live elsewhere.
    match fs::remove_file(directory.join("ssh-connections.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retires_only_legacy_profile_and_never_follows_links() {
        let root = std::env::temp_dir().join(format!("lra-retire-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("ssh-connections.json"), "private fixture").unwrap();
        fs::write(root.join("identity.json"), "retained fixture").unwrap();
        retire(&root).unwrap();
        retire(&root).unwrap();
        assert!(!root.join("ssh-connections.json").exists());
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.join("identity.json"),
            root.join("ssh-connections.json"),
        )
        .unwrap();
        retire(&root).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("identity.json")).unwrap(),
            "retained fixture"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
