//! One owner for a compositor lease. Metadata snapshots never block frame pulls;
//! foreground commands retain the same authenticated bridge and take priority
//! between snapshot queries. Only one bounded snapshot may be outstanding.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
};

type Reply = std::result::Result<Value, String>;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Metadata {
    Scope,
    Scene,
}
enum Work {
    Call(Value, SyncSender<Reply>),
    Snapshot(Metadata, SyncSender<Reply>),
}
pub(super) struct AsyncRpc {
    sender: Option<SyncSender<Work>>,
    pending: Option<(Metadata, Receiver<Reply>)>,
    ready: Arc<AtomicBool>,
    cancel: UnixStream,
    worker: Option<JoinHandle<()>>,
}
impl AsyncRpc {
    pub fn new(
        mut call: impl FnMut(Value) -> Result<Value> + Send + 'static,
        cancel: UnixStream,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let (sender, requests) = mpsc::sync_channel(16);
        let ready = Arc::new(AtomicBool::new(false));
        let completed = ready.clone();
        let worker = thread::Builder::new()
            .name("wayland-metadata".into())
            .spawn(move || {
                let invoke = |call: &mut dyn FnMut(Value) -> Result<Value>, value| {
                    call(value).map_err(|e| e.to_string())
                };
                let mut deferred = None;
                loop {
                    let work = match deferred
                        .take()
                        .map(|(kind, reply)| Work::Snapshot(kind, reply))
                        .or_else(|| requests.recv().ok())
                    {
                        Some(work) => work,
                        None => break,
                    };
                    match work {
                        Work::Call(value, reply) => {
                            let _ = reply.send(invoke(&mut call, value));
                        }
                        Work::Snapshot(kind, reply) => {
                            let mut snapshot = json!({});
                            let result = (|| -> Reply {
                                let ops: &[&str] = match kind {
                                    Metadata::Scope => &["scope"],
                                    Metadata::Scene => &["scope", "scene", "list"],
                                };
                                for op in ops {
                                    // Input arriving during a metadata RPC runs before
                                    // the next query, never behind an entire scan.
                                    while let Ok(work) = requests.try_recv() {
                                        match work {
                                            Work::Call(value, reply) => {
                                                let _ = reply.send(invoke(&mut call, value));
                                            }
                                            // Resume can invalidate an in-flight snapshot.
                                            // Keep its replacement instead of dropping it
                                            // while draining foreground commands.
                                            Work::Snapshot(kind, reply) => {
                                                deferred = Some((kind, reply))
                                            }
                                        }
                                    }
                                    snapshot[op] = invoke(&mut call, json!({"op":op}))?;
                                }
                                Ok(match kind {
                                    Metadata::Scope => snapshot["scope"].take(),
                                    Metadata::Scene => snapshot,
                                })
                            })();
                            // Mark before enqueue: the owner may consume and
                            // clear readiness immediately after send succeeds.
                            completed.store(true, Ordering::Release);
                            if reply.send(result).is_ok() {
                                notify();
                            } else {
                                completed.store(false, Ordering::Release);
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            pending: None,
            ready,
            cancel,
            worker: Some(worker),
        })
    }
    pub fn call(&mut self, value: Value) -> Result<Value> {
        let (sender, reply) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .context("wayland_bridge_unavailable")?
            .send(Work::Call(value, sender))?;
        reply
            .recv()
            .context("wayland_bridge_unavailable")?
            .map_err(anyhow::Error::msg)
    }
    pub fn invalidate_snapshot(&mut self) {
        self.pending.take();
        self.ready.store(false, Ordering::Release);
    }
    pub fn metadata_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn poll_snapshot(&mut self) -> Result<Option<Reply>> {
        self.poll_metadata(Metadata::Scene)
    }
    pub fn poll_scope(&mut self) -> Result<Option<Reply>> {
        self.poll_metadata(Metadata::Scope)
    }
    fn poll_metadata(&mut self, kind: Metadata) -> Result<Option<Reply>> {
        if self.pending.as_ref().is_some_and(|(old, _)| *old != kind) {
            self.invalidate_snapshot();
        }
        if let Some((_, reply)) = &self.pending {
            match reply.try_recv() {
                Ok(value) => {
                    self.pending = None;
                    self.ready.store(false, Ordering::Release);
                    return Ok(Some(value));
                }
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => bail!("wayland_bridge_unavailable"),
            }
        }
        let (sender, reply) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .context("wayland_bridge_unavailable")?
            .try_send(Work::Snapshot(kind, sender))?;
        self.pending = Some((kind, reply));
        Ok(None)
    }
}
impl Drop for AsyncRpc {
    fn drop(&mut self) {
        // Interrupt an in-flight read before joining; the bridge's Drop then
        // terminates its helper and releases the D-Bus-owned compositor lease.
        self.sender.take();
        let _ = self.cancel.shutdown(std::net::Shutdown::Both);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Condvar, Mutex};
    #[test]
    fn ordinary_scope_is_nonblocking_and_resize_discards_old_geometry() -> Result<()> {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let blocked = gate.clone();
        let (entered, started) = mpsc::sync_channel(1);
        let (notified, notification) = mpsc::sync_channel(1);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let calls = seen.clone();
        let (cancel, _other) = UnixStream::pair()?;
        let (mut generation, mut width) = (0, 400);
        let mut rpc = AsyncRpc::new(
            move |value| {
                let op = value["op"].as_str().unwrap();
                calls.lock().unwrap().push(op.to_string());
                if op == "resize" {
                    width = 800;
                    return Ok(json!({"ok":true}));
                }
                assert_eq!(op, "scope", "ordinary windows need no scene/list queries");
                generation += 1;
                let snapshot = json!([{"generation":generation,"width":width}]);
                if generation == 1 {
                    entered.send(()).unwrap();
                    let (lock, cv) = &*blocked;
                    let _ = cv
                        .wait_timeout_while(
                            lock.lock().unwrap(),
                            std::time::Duration::from_secs(2),
                            |ready| !*ready,
                        )
                        .unwrap();
                }
                Ok(snapshot)
            },
            cancel,
            move || {
                notified.send(()).unwrap();
            },
        )?;
        assert!(rpc.poll_scope()?.is_none());
        started.recv_timeout(std::time::Duration::from_secs(1))?;
        let start = std::time::Instant::now();
        for _ in 0..100 {
            assert!(rpc.poll_scope()?.is_none());
        }
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
        rpc.invalidate_snapshot();
        let (reply, result) = mpsc::sync_channel(1);
        rpc.sender
            .as_ref()
            .unwrap()
            .send(Work::Call(json!({"op":"resize"}), reply))?;
        assert!(rpc.poll_scope()?.is_none());
        *gate.0.lock().unwrap() = true;
        gate.1.notify_one();
        result
            .recv_timeout(std::time::Duration::from_secs(1))?
            .unwrap();
        notification.recv_timeout(std::time::Duration::from_secs(1))?;
        assert!(rpc.metadata_ready());
        let fresh = rpc.poll_scope()?.unwrap().unwrap();
        assert_eq!(fresh, json!([{"generation":2,"width":800}]));
        assert!(!rpc.metadata_ready());
        assert_eq!(*seen.lock().unwrap(), ["scope", "resize", "scope"]);
        Ok(())
    }
    #[test]
    fn blocked_metadata_keeps_polling_nonblocking_and_prioritizes_control() -> Result<()> {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let blocked = gate.clone();
        let (entered, started) = mpsc::sync_channel(1);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let calls = seen.clone();
        let mut generation = 0;
        let (cancel, _other) = UnixStream::pair()?;
        let mut rpc = AsyncRpc::new(
            move |value| {
                let op = value["op"].as_str().unwrap().to_string();
                calls.lock().unwrap().push(op.clone());
                if op == "scope" {
                    generation += 1;
                    entered.send(()).unwrap();
                    let (lock, cv) = &*blocked;
                    let _guard = cv
                        .wait_while(lock.lock().unwrap(), |ready| !*ready)
                        .unwrap();
                }
                Ok(json!({"op":op, "generation":generation}))
            },
            cancel,
            || {},
        )?;
        assert!(rpc.poll_snapshot()?.is_none());
        started.recv()?;
        // No lock on the bridge and no second request while a scan is blocked.
        for _ in 0..100 {
            assert!(rpc.poll_snapshot()?.is_none());
        }
        rpc.invalidate_snapshot();
        assert!(rpc.poll_snapshot()?.is_none());
        let (reply, result) = mpsc::sync_channel(1);
        rpc.sender
            .as_ref()
            .unwrap()
            .send(Work::Call(json!({"op":"scene_point"}), reply))?;
        *gate.0.lock().unwrap() = true;
        gate.1.notify_one();
        assert_eq!(result.recv()?.unwrap()["op"], "scene_point");
        let fresh = rpc.pending.as_ref().unwrap().1.recv()?.unwrap();
        assert_eq!(
            fresh["scope"]["generation"], 2,
            "resume must never publish the discarded producer's metadata"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "scope",
                "scene_point",
                "scene",
                "list",
                "scope",
                "scene",
                "list"
            ]
        );
        Ok(())
    }
}
