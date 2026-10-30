//! Keep slow receivers from accumulating seconds of dependent video frames.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq)]
pub enum Ready {
    Send,
    Wait,
    Restart,
}
#[derive(Default)]
pub struct VideoFlow {
    enabled: bool,
    next: u64,
    pending: VecDeque<(u64, Instant)>,
}
impl VideoFlow {
    pub fn enable(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.pending.clear();
    }
    pub fn ready(&mut self, now: Instant) -> Ready {
        if !self.enabled {
            return Ready::Send;
        }
        if self
            .pending
            .front()
            .is_some_and(|(_, sent)| now.duration_since(*sent) >= Duration::from_millis(500))
        {
            self.pending.clear();
            return Ready::Restart;
        }
        // Allow the Mac host's 50 ms feedback cadence plus ~30 ms RTT at 60 fps.
        // Bound stalled display/network queues without throttling healthy feedback.
        if self.pending.len() >= 6 {
            Ready::Wait
        } else {
            Ready::Send
        }
    }
    pub fn sent(&mut self, now: Instant) -> Option<u64> {
        if !self.enabled {
            return None;
        }
        self.next = self.next.checked_add(1).expect("video frame id exhausted");
        self.pending.push_back((self.next, now));
        Some(self.next)
    }
    pub fn acknowledge(&mut self, id: u64) {
        // Cumulative presentation acknowledgements may skip intermediate frames.
        if !self.pending.iter().any(|(sent, _)| *sent == id) {
            return;
        }
        while self.pending.front().is_some_and(|(sent, _)| *sent <= id) {
            self.pending.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_backlog_and_recovers_without_dropping_coded_references() {
        let now = Instant::now();
        let mut flow = VideoFlow::default();
        flow.enable(true);
        for _ in 0..6 {
            assert_eq!(flow.ready(now), Ready::Send);
            flow.sent(now);
        }
        assert_eq!(flow.ready(now), Ready::Wait);
        flow.acknowledge(999);
        assert_eq!(flow.ready(now), Ready::Wait);
        flow.acknowledge(3);
        assert_eq!(flow.ready(now), Ready::Send);
        flow.sent(now);
        assert_eq!(flow.ready(now + Duration::from_millis(501)), Ready::Restart);
        assert_eq!(flow.ready(now + Duration::from_millis(502)), Ready::Send);
        assert_eq!(flow.sent(now), Some(8));
        flow.acknowledge(4);
        assert_eq!(flow.pending.len(), 1);
    }
    #[test]
    fn legacy_clients_do_not_need_acknowledgements() {
        let now = Instant::now();
        let mut flow = VideoFlow::default();
        for _ in 0..100 {
            assert_eq!(flow.ready(now), Ready::Send);
            assert_eq!(flow.sent(now), None);
        }
    }
}
