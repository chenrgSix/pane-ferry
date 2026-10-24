//! A slow window cannot block the publisher's control/other-window reader.
//! After losing a coded frame, discard its dependent frames until an IDR.
use crate::framing::Packet;
use std::sync::Arc;
use tokio::{
    sync::{mpsc, Semaphore},
    task::AbortHandle,
};

pub(crate) struct MediaQueue {
    pub serial: u64,
    pub sender: Option<mpsc::Sender<Packet>>,
    pub worker: Option<AbortHandle>,
    waiting_keyframe: bool,
}
impl Default for MediaQueue {
    fn default() -> Self {
        Self {
            serial: 0,
            sender: None,
            worker: None,
            waiting_keyframe: true,
        }
    }
}
impl MediaQueue {
    pub fn needs_keyframe(&self) -> bool {
        self.waiting_keyframe
    }
    pub fn accepts(&self, packet: &Packet) -> bool {
        !self.waiting_keyframe || packet.value["key_frame"] == true
    }
    pub fn failed(&mut self) {
        self.sender = None;
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
        self.waiting_keyframe = true;
    }
    pub fn enqueue(&mut self, mut packet: Packet, budget: &Arc<Semaphore>) {
        if !self.accepts(&packet) {
            return;
        }
        if !packet.try_reserve(budget) {
            self.waiting_keyframe = true;
            return;
        }
        match self.sender.as_ref().map(|sender| sender.try_send(packet)) {
            Some(Ok(())) => self.waiting_keyframe = false,
            Some(Err(mpsc::error::TrySendError::Closed(_))) | None => self.failed(),
            Some(Err(mpsc::error::TrySendError::Full(_))) => self.waiting_keyframe = true,
        }
    }
}
impl Drop for MediaQueue {
    fn drop(&mut self) {
        self.failed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn frame(n: u64, key: bool) -> Packet {
        Packet::new(
            json!({"type":"frame","codec":"h264","data":"YQ==","seq":n,"key_frame":key}),
            1,
        )
        .unwrap()
    }
    #[tokio::test]
    async fn slow_window_drops_dependencies_until_a_keyframe_and_releases_quota() {
        let budget = Arc::new(Semaphore::new(1024));
        let (tx, mut rx) = mpsc::channel(2);
        let mut slot = MediaQueue::default();
        slot.sender = Some(tx);
        slot.enqueue(frame(1, true), &budget);
        slot.enqueue(frame(2, false), &budget);
        slot.enqueue(frame(3, false), &budget); // full: 3 and its dependents cannot be decoded
        assert_eq!(rx.recv().await.unwrap().value["seq"], 1);
        slot.enqueue(frame(4, false), &budget); // space is available, still reject a dependent
        assert_eq!(rx.recv().await.unwrap().value["seq"], 2);
        assert!(rx.try_recv().is_err());
        slot.enqueue(frame(5, true), &budget);
        assert_eq!(rx.recv().await.unwrap().value["seq"], 5);
        assert_eq!(budget.available_permits(), 1024);
    }
    #[tokio::test]
    async fn destroying_a_window_releases_a_blocked_worker_and_its_buffer() {
        let budget = Arc::new(Semaphore::new(1024));
        let (tx, mut rx) = mpsc::channel(2);
        let task = tokio::spawn(async move {
            let _packet = rx.recv().await;
            std::future::pending::<()>().await;
        });
        let mut slot = MediaQueue::default();
        slot.sender = Some(tx);
        slot.worker = Some(task.abort_handle());
        slot.enqueue(frame(1, true), &budget);
        tokio::task::yield_now().await;
        assert!(budget.available_permits() < 1024);
        drop(slot);
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(budget.available_permits(), 1024);
    }
}
