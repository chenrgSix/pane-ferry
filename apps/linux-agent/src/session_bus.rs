//! A per-session view of the user's bus exposing only Secret Service.
//! Do not replace isolated buses globally: desktop activation could otherwise
//! forward a new app into another session or the user's physical desktop.
use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::Read,
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        io::AsRawFd,
        net::UnixStream,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

pub struct SecretsBus {
    child: Child,
    path: PathBuf,
    // xdg-dbus-proxy exits on EOF, including when the Agent crashes.
    _lifetime: UnixStream,
}

impl SecretsBus {
    pub fn start(runtime: &Path, session: &str) -> Result<Self> {
        let uid = unsafe { libc::geteuid() };
        let desktop = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")))
            .join("bus");
        let metadata = fs::metadata(&desktop)?;
        if !desktop.is_absolute() || !metadata.file_type().is_socket() || metadata.uid() != uid {
            bail!("desktop session bus unavailable")
        }
        let path = runtime.join(format!("secrets-{session}.sock"));
        let (lifetime, ready) = UnixStream::pair()?;
        let descriptor = ready.as_raw_fd();
        let mut command = Command::new("xdg-dbus-proxy");
        command
            .arg(format!("--fd={descriptor}"))
            .arg(address(&desktop))
            .arg(&path)
            .args(["--filter", "--talk=org.freedesktop.secrets"])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Only the lifetime descriptor crosses exec; the parent's end stays CLOEXEC.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(descriptor, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().context("xdg-dbus-proxy unavailable")?;
        drop(ready);
        let mut bus = Self {
            child,
            path,
            _lifetime: lifetime,
        };
        bus._lifetime
            .set_read_timeout(Some(Duration::from_secs(5)))?;
        bus._lifetime.read_exact(&mut [0u8])?;
        if bus.child.try_wait()?.is_some() || !fs::metadata(&bus.path)?.file_type().is_socket() {
            bail!("desktop session bus proxy unavailable")
        }
        Ok(bus)
    }

    pub fn address(&self) -> String {
        address(&self.path)
    }
}

fn address(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut value = String::from("unix:path=");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte) {
            value.push(byte as char);
        } else {
            value.push_str(&format!("%{byte:02x}"));
        }
    }
    value
}

impl Drop for SecretsBus {
    fn drop(&mut self) {
        crate::stop_child(&mut self.child);
        let _ = fs::remove_file(&self.path);
    }
}
