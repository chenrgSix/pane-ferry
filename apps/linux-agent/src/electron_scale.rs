//! Explicit, target-only ChatGPT CSS zoom. The child restores on parent EOF.
use crate::{process_identity, ProcessIdentity};
use anyhow::{bail, Context, Result};
use paneferry_protocol::read_line_limit;
use serde_json::{json, Value};
use std::{
    io::{BufReader, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

const HELPER: &str = include_str!("electron_page_scale.py");
pub const EXECUTABLE: &str = "/usr/lib/chatgpt/ChatGPT";

pub fn prepare(pid: &str) -> Result<()> {
    let pid: u32 = pid.parse()?;
    let status = Command::new("/usr/bin/python3")
        .args(["-I", "-u", "-c", HELPER, "--prepare", &pid.to_string()])
        .status()?;
    if !status.success() {
        bail!("page_scale_restart_required")
    }
    Ok(())
}

pub struct Lease {
    pub window: u32,
    pub size: [u32; 2],
    child: Option<Child>,
    wire: BufReader<UnixStream>,
    stopped: bool,
}
impl Lease {
    pub fn start(
        identity: &ProcessIdentity,
        window: u32,
        display: u16,
        authority: &Path,
    ) -> Result<Self> {
        if identity.executable != Path::new(EXECUTABLE)
            || process_identity(identity.pid).ok().as_ref() != Some(identity)
        {
            bail!("page_scale_unsupported")
        }
        let (parent, child) = UnixStream::pair()?;
        parent.set_read_timeout(Some(Duration::from_secs(12)))?;
        parent.set_write_timeout(Some(Duration::from_secs(2)))?;
        let input: OwnedFd = child.try_clone()?.into();
        let output: OwnedFd = child.into();
        let child = Command::new("/usr/bin/python3")
            .args(["-I", "-u", "-c", HELPER])
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut lease = Self {
            window,
            size: [0, 0],
            child: Some(child),
            wire: BufReader::new(parent),
            stopped: false,
        };
        let r = lease.call(
            json!({"op":"start", "pid":identity.pid,"started":identity.started,
            "window_id":window,"display":format!(":{display}"),"authority":authority}),
        )?;
        lease.size = serde_json::from_value(r["size"].clone())?;
        paneferry_protocol::dimensions(lease.size[0], lease.size[1])?;
        Ok(lease)
    }
    fn call(&mut self, request: Value) -> Result<Value> {
        serde_json::to_writer(self.wire.get_mut(), &request)?;
        self.wire.get_mut().write_all(b"\n")?;
        let line = read_line_limit(&mut self.wire, 4096)?.context("page_scale_unavailable")?;
        let answer: Value = serde_json::from_slice(&line)?;
        if let Some(code) = answer["error"].as_str() {
            bail!("{}", code)
        }
        Ok(answer)
    }
    pub fn commit(&mut self) -> Result<()> {
        if self.call(json!({"op":"commit"}))?["active"] != true {
            bail!("page_scale_resize_failed")
        }
        Ok(())
    }
    pub fn check(&mut self) -> Result<()> {
        if self
            .child
            .as_mut()
            .context("page_scale_unavailable")?
            .try_wait()?
            .is_some()
        {
            bail!("page_scale_unavailable")
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let restored = self.call(json!({"op":"stop"}));
        let _ = self.wire.get_mut().shutdown(std::net::Shutdown::Both);
        // EOF triggers independent CSS/geometry recovery even on publisher death.
        // Never kill the helper halfway through restoration.
        if let Some(mut child) = self.child.take() {
            // Reap even after a broken pipe/timeout; let restoration finish.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        if restored?["restored"] != true {
            bail!("page_scale_restore_incomplete")
        }
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("page scale recovery: {error}");
        }
    }
}
