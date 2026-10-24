#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub use tokio::net::{UnixListener as LocalListener, UnixStream as LocalStream};
#[cfg(windows)]
pub use windows::{bind, same_user, LocalListener, LocalStream};

#[cfg(unix)]
pub fn bind(path: &std::path::Path) -> std::io::Result<LocalListener> {
    use std::{
        fs,
        os::unix::fs::{FileTypeExt, PermissionsExt},
    };
    if let Ok(info) = path.symlink_metadata() {
        if !info.file_type().is_socket() {
            return Err(std::io::Error::other("invalid socket"));
        }
        fs::remove_file(path)?;
    }
    let listener = LocalListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(unix)]
pub fn same_user(stream: &LocalStream) -> bool {
    stream
        .peer_cred()
        .is_ok_and(|cred| cred.uid() == unsafe { libc::geteuid() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn private_duplex_connections_and_reconnect() {
        let root = tempfile::tempdir().unwrap();
        crate::storage::private_directory(root.path()).unwrap();
        let path = root.path().join("node.sock");
        let listener = bind(&path).unwrap();
        for _ in 0..2 {
            let connect = LocalStream::connect(&path);
            let (client, server) = tokio::join!(connect, listener.accept());
            let mut client = client.unwrap();
            let (mut server, _) = server.unwrap();
            assert!(same_user(&server));
            client.write_all(b"request").await.unwrap();
            let mut data = [0; 7];
            server.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"request");
            server.write_all(b"response").await.unwrap();
            let mut data = [0; 8];
            client.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"response");
        }
    }
}
