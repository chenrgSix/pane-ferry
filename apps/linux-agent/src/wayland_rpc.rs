//! One owner for a compositor lease. Metadata snapshots never block frame pulls;
//! foreground commands retain the same authenticated bridge and take priority
//! between snapshot queries. Only one bounded snapshot may be outstanding.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    os::unix::net::UnixStream,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread::{self, JoinHandle},
};

type Reply = std::result::Result<Value, String>;
enum Work {
    Call(Value, SyncSender<Reply>),
    Snapshot(SyncSender<Reply>),
}
pub(super) struct AsyncRpc {
    sender: Option<SyncSender<Work>>,
    pending: Option<Receiver<Reply>>,
    cancel: UnixStream,
    worker: Option<JoinHandle<()>>,
}
impl AsyncRpc {
    pub fn new(
        mut call: impl FnMut(Value) -> Result<Value> + Send + 'static,
        cancel: UnixStream,
    ) -> Result<Self> {
        let (sender, requests) = mpsc::sync_channel(16);
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
                        .map(Work::Snapshot)
                        .or_else(|| requests.recv().ok())
                    {
                        Some(work) => work,
                        None => break,
                    };
                    match work {
                        Work::Call(value, reply) => {
                            let _ = reply.send(invoke(&mut call, value));
                        }
                        Work::Snapshot(reply) => {
                            let mut snapshot = json!({});
                            let result = (|| -> Reply {
                                for op in ["scope", "scene", "list"] {
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
                                            Work::Snapshot(reply) => deferred = Some(reply),
                                        }
                                    }
                                    snapshot[op] = invoke(&mut call, json!({"op":op}))?;
                                }
                                Ok(snapshot)
                            })();
                            let _ = reply.send(result);
                        }
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            pending: None,
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
    }
    pub fn poll_snapshot(&mut self) -> Result<Option<Reply>> {
        if let Some(reply) = &self.pending {
            match reply.try_recv() {
                Ok(value) => {
                    self.pending = None;
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
            .try_send(Work::Snapshot(sender))?;
        self.pending = Some(reply);
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
        let fresh = rpc.pending.as_ref().unwrap().recv()?.unwrap();
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
