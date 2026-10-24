use crate::security::{same_process_user, Descriptor};
use sha2::{Digest, Sha256};
use std::{
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
    sync::Mutex,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};

fn name(path: &Path) -> io::Result<String> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing IPC parent"))?
        .canonicalize()?;
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing IPC name"))?;
    use std::os::windows::ffi::OsStrExt;
    let bytes: Vec<_> = parent
        .join(filename)
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    Ok(format!(r"\\.\pipe\paneferry-{:x}", Sha256::digest(bytes)))
}
fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    let descriptor = Descriptor::new()?;
    let mut attributes = descriptor.attributes();
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                (&mut attributes as *mut windows_sys::Win32::Security::SECURITY_ATTRIBUTES).cast(),
            )
    }
}
pub struct LocalListener {
    name: String,
    pending: Mutex<NamedPipeServer>,
}
pub fn bind(path: &Path) -> io::Result<LocalListener> {
    let name = name(path)?;
    let pending = Mutex::new(create(&name, true)?);
    Ok(LocalListener { name, pending })
}
impl LocalListener {
    pub async fn accept(&self) -> io::Result<(LocalStream, ())> {
        let mut pending = self.pending.lock().await;
        pending.connect().await?;
        let next = create(&self.name, false)?;
        let stream = std::mem::replace(&mut *pending, next);
        Ok((LocalStream::Server(stream), ()))
    }
}
pub enum LocalStream {
    Server(NamedPipeServer),
    Client(NamedPipeClient),
}
impl LocalStream {
    pub async fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
        let name = name(path.as_ref())?;
        for _ in 0..100 {
            match ClientOptions::new().open(&name) {
                Ok(client) => {
                    let mut pid = 0;
                    if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle(), &mut pid) } == 0
                        || !same_process_user(pid)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "invalid pipe server",
                        ));
                    }
                    return Ok(Self::Client(client));
                }
                Err(error) if error.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "local pipe busy"))
    }
    pub fn into_split(self) -> (tokio::io::ReadHalf<Self>, tokio::io::WriteHalf<Self>) {
        tokio::io::split(self)
    }
}
pub fn same_user(stream: &LocalStream) -> bool {
    let mut pid = 0;
    let result = unsafe {
        match stream {
            LocalStream::Server(pipe) => {
                GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid)
            }
            LocalStream::Client(pipe) => {
                GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid)
            }
        }
    };
    result != 0 && same_process_user(pid)
}
impl AsyncRead for LocalStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(p) => Pin::new(p).poll_read(cx, buf),
            Self::Client(p) => Pin::new(p).poll_read(cx, buf),
        }
    }
}
impl AsyncWrite for LocalStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Server(p) => Pin::new(p).poll_write(cx, buf),
            Self::Client(p) => Pin::new(p).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(p) => Pin::new(p).poll_flush(cx),
            Self::Client(p) => Pin::new(p).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(p) => Pin::new(p).poll_shutdown(cx),
            Self::Client(p) => Pin::new(p).poll_shutdown(cx),
        }
    }
}
