//! One input-only portal/EIS connection for the Agent's graphical desktop.
//! A disconnected controller releases its inputs, not the shared permission.
use anyhow::{bail, Context, Result};
use remoteapp_backend_x11::DesktopInput;
use remoteapp_protocol::read_line_limit;
use serde_json::{json, Value};
use std::{
    io::{BufReader, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub struct Manager(Arc<Inner>);
struct Inner {
    runtime: PathBuf,
    broker: Mutex<Option<Broker>>,
    next: AtomicU64,
}
struct Broker {
    child: Child,
    stream: BufReader<UnixStream>,
    status: &'static str,
    checked: Instant,
}
struct Lease {
    inner: Arc<Inner>,
    owner: u64,
}
impl Manager {
    pub fn new(runtime: PathBuf) -> Self {
        Self(Arc::new(Inner {
            runtime,
            broker: Mutex::new(None),
            next: AtomicU64::new(1),
        }))
    }
    pub fn lease(&self) -> Arc<dyn DesktopInput> {
        Arc::new(Lease {
            inner: self.0.clone(),
            owner: self.0.next.fetch_add(1, Ordering::Relaxed),
        })
    }
}
impl Broker {
    fn start(runtime: &std::path::Path) -> Result<Self> {
        let (parent, child) = UnixStream::pair()?;
        parent.set_read_timeout(Some(Duration::from_secs(2)))?;
        parent.set_write_timeout(Some(Duration::from_secs(2)))?;
        let input: OwnedFd = child.try_clone()?.into();
        let output: OwnedFd = child.into();
        let child = Command::new("/usr/bin/python3")
            .args(["-I", "-u", "-c", include_str!("desktop_input.py")])
            .arg(runtime)
            .arg(std::env::current_exe()?)
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            child,
            stream: BufReader::new(parent),
            status: "pending",
            checked: Instant::now(),
        })
    }
    fn call(&mut self, request: Value) -> Result<&'static str> {
        serde_json::to_writer(self.stream.get_mut(), &request)?;
        self.stream.get_mut().write_all(b"\n")?;
        let line = read_line_limit(&mut self.stream, 4096)?.context("input broker ended")?;
        let value: Value = serde_json::from_slice(&line)?;
        let status = match value["status"].as_str() {
            Some("ready") => "ready",
            Some("pending") => "pending",
            Some("denied") => "denied",
            Some("paused") => "paused",
            _ => "unavailable",
        };
        self.status = status;
        self.checked = Instant::now();
        if let Some(error) = value["error"].as_str() {
            bail!(
                "{}",
                match error {
                    "desktop_authorization_pending" => "desktop_authorization_pending",
                    "desktop_authorization_denied" => "desktop_authorization_denied",
                    "desktop_input_paused" => "desktop_input_paused",
                    _ => "desktop_input_unavailable",
                }
            );
        }
        Ok(status)
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        // EOF normally lets the helper release its devices and close the portal.
        let _ = self.stream.get_mut().shutdown(std::net::Shutdown::Both);
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
impl Lease {
    fn call(&self, request: Value, create: bool) -> Result<&'static str> {
        let mut slot = self.inner.broker.lock().unwrap();
        if slot
            .as_mut()
            .is_some_and(|b| !matches!(b.child.try_wait(), Ok(None)))
        {
            *slot = None;
        }
        if slot.is_none() {
            if !create {
                return Ok("unavailable");
            }
            *slot = Some(
                Broker::start(&self.inner.runtime)
                    .map_err(|_| anyhow::anyhow!("desktop_input_unavailable"))?,
            );
        }
        let broker = slot.as_mut().unwrap();
        let result = broker.call(request);
        if result
            .as_ref()
            .is_err_and(|e| !e.to_string().starts_with("desktop_"))
        {
            // Never replay a possibly delivered event after an ambiguous failure.
            *slot = None;
            bail!("desktop_input_unavailable")
        }
        result
    }
}
impl DesktopInput for Lease {
    fn prepare(&self) {
        let _ = self.call(json!({"type":"ensure"}), true);
    }
    fn status(&self) -> &'static str {
        {
            let slot = self.inner.broker.lock().unwrap();
            if let Some(broker) = &*slot {
                if broker.checked.elapsed() < Duration::from_millis(50) {
                    return broker.status;
                }
            } else {
                return "unavailable";
            }
        }
        self.call(json!({"type":"status"}), false)
            .unwrap_or("unavailable")
    }
    fn pointer(&self, x: i32, y: i32, button: u8, pressed: bool) -> Result<()> {
        self.call(json!({"type":"pointer","owner":self.owner,"x":x,"y":y,"button":button,"pressed":pressed}), pressed || button == 0)?;
        Ok(())
    }
    fn key(&self, code: u8, pressed: bool) -> Result<()> {
        self.call(
            json!({"type":"key","owner":self.owner,"code":code,"pressed":pressed}),
            pressed,
        )?;
        Ok(())
    }
    fn release_all(&self) {
        let _ = self.call(json!({"type":"release","owner":self.owner}), false);
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Manager, std::thread::JoinHandle<Vec<Value>>) {
        let (parent, mut peer) = UnixStream::pair().unwrap();
        parent
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        // A live child pins the Broker lifetime; all protocol traffic stays in
        // this test-owned socket pair, without starting a portal or touching X11.
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let manager = Manager::new(PathBuf::from("/unused-fixture"));
        *manager.0.broker.lock().unwrap() = Some(Broker {
            child,
            stream: BufReader::new(parent),
            status: "ready",
            checked: Instant::now(),
        });
        let reader = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let mut reader = BufReader::new(peer.try_clone().unwrap());
            while let Some(line) = read_line_limit(&mut reader, 4096).unwrap() {
                requests.push(serde_json::from_slice(&line).unwrap());
                peer.write_all(b"{\"status\":\"ready\"}\n").unwrap();
            }
            requests
        });
        (manager, reader)
    }
    #[test]
    fn applications_and_reconnect_keep_one_broker_with_distinct_owners() {
        let (manager, reader) = fixture();
        {
            let first = manager.lease();
            let second = manager.lease();
            first.prepare();
            first.key(38, true).unwrap();
            second.key(38, true).unwrap();
            first.release_all();
            first.prepare(); // Controller reconnect retains its input lease.
            second.pointer(100, 200, 1, true).unwrap();
        }
        drop(manager);
        let requests = reader.join().unwrap();
        assert_eq!(requests[1]["owner"], 1);
        assert_eq!(requests[2]["owner"], 2);
        assert_eq!(requests[3], json!({"type":"release","owner":1}));
        assert_eq!(requests[4], json!({"type":"ensure"}));
        assert_eq!(requests[5]["type"], "pointer");
        assert_eq!(requests[5]["owner"], 2);
        assert_eq!(
            requests.iter().filter(|r| r["type"] == "release").count(),
            3
        );
    }
    #[test]
    fn disconnect_cleanup_does_not_start_or_authorize_a_helper() {
        let manager = Manager::new(PathBuf::from("/unused-fixture"));
        let lease = manager.lease();
        lease.release_all();
        drop(lease);
        assert!(manager.0.broker.lock().unwrap().is_none());
    }
}
