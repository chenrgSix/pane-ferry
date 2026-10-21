use anyhow::{bail, Result};
use remoteapp_backend_x11::CapturedImage;
use std::{
    io::{Read, Write},
    os::fd::OwnedFd,
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    sync::OnceLock,
    time::Duration,
};
const WORKER: &str = include_str!("video_encoder.py");
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("/usr/bin/python3")
            .args(["-c", WORKER, "--check"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}
pub struct Encoder {
    process: Child,
    wire: UnixStream,
    pub width: u32,
    pub height: u32,
    pub stream: u64,
}
pub struct Packet {
    pub bytes: Vec<u8>,
    pub keyframe: bool,
}
impl Encoder {
    pub fn new(width: u32, height: u32, stream: u64) -> Result<Self> {
        remoteapp_protocol::dimensions(width, height)?;
        let (wire, child) = UnixStream::pair()?;
        wire.set_read_timeout(Some(Duration::from_secs(3)))?;
        wire.set_write_timeout(Some(Duration::from_secs(3)))?;
        let process = Command::new("/usr/bin/python3")
            .args(["-u", "-c", WORKER, &width.to_string(), &height.to_string()])
            .stdin(Stdio::from(OwnedFd::from(child.try_clone()?)))
            .stdout(Stdio::from(OwnedFd::from(child)))
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            process,
            wire,
            width,
            height,
            stream,
        })
    }
    pub fn encode(&mut self, image: &CapturedImage) -> Result<Packet> {
        if image.width != self.width || image.height != self.height {
            bail!("video geometry changed")
        }
        let pixels = image.bgrx()?;
        self.wire.write_all(&(pixels.len() as u32).to_be_bytes())?;
        self.wire.write_all(&pixels)?;
        let mut header = [0; 5];
        self.wire.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header[..4].try_into()?) as usize;
        if length == 0 || length > 12 * 1024 * 1024 || header[4] > 1 {
            bail!("invalid encoded video packet")
        }
        let mut bytes = vec![0; length];
        self.wire.read_exact(&mut bytes)?;
        Ok(Packet {
            bytes,
            keyframe: header[4] == 1,
        })
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.wire.shutdown(std::net::Shutdown::Both);
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
