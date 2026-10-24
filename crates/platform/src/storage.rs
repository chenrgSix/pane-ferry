#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{fs, io, path::Path};

pub fn private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let info = fs::symlink_metadata(path)?;
    if !info.file_type().is_dir() || is_reparse(&info) {
        return Err(io::Error::other("invalid private directory"));
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    #[cfg(windows)]
    crate::security::protect_directory(path)?;
    Ok(())
}

pub fn options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    options.mode(0o600);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Refuse to follow a reparse point when opening identity/lock files.
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

pub fn check_private_file(path: &Path) -> io::Result<()> {
    let info = fs::symlink_metadata(path)?;
    if !info.is_file() || is_reparse(&info) || info.len() >= 1024 * 1024 {
        return Err(io::Error::other("invalid private file"));
    }
    #[cfg(unix)]
    if info.uid() != unsafe { libc::geteuid() } || info.mode() & 0o077 != 0 {
        return Err(io::Error::other("unsafe private file permissions"));
    }
    #[cfg(windows)]
    crate::security::check_private(path)?;
    Ok(())
}

pub fn is_reparse(info: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        info.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        info.file_type().is_symlink()
    }
}

pub fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::rename(source, destination)?;
        fs::File::open(
            destination
                .parent()
                .ok_or_else(|| io::Error::other("missing parent"))?,
        )?
        .sync_all()
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        if MoveFileExW(
            crate::security::wide(source).as_ptr(),
            crate::security::wide(destination).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_file_can_be_atomically_replaced() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        private_directory(root.path()).unwrap();
        let target = root.path().join("identity");
        options()
            .write(true)
            .create_new(true)
            .open(&target)
            .unwrap()
            .write_all(b"old")
            .unwrap();
        let next = root.path().join("next");
        options()
            .write(true)
            .create_new(true)
            .open(&next)
            .unwrap()
            .write_all(b"new")
            .unwrap();
        replace(&next, &target).unwrap();
        check_private_file(&target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }
    #[cfg(unix)]
    #[test]
    fn rejects_public_identity_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("key");
        fs::write(&file, b"secret").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check_private_file(&file).is_err());
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(check_private_file(&link).is_err());
    }
}
