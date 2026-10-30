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
    baseline: Option<Duration>,
    quality: u32,
    bad_samples: u8,
    stable_since: Option<Instant>,
    last_adjusted: Option<Instant>,
}
impl VideoFlow {
    pub fn enable(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.pending.clear();
        self.baseline = None;
        self.quality = 1000;
        self.bad_samples = 0;
        self.stable_since = None;
        self.last_adjusted = None;
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
    pub fn acknowledge(&mut self, id: u64) -> Option<u32> {
        self.acknowledge_at(id, Instant::now())
    }
    fn acknowledge_at(&mut self, id: u64, now: Instant) -> Option<u32> {
        // Cumulative ACKs may skip frames. Unknown/stale ACKs affect nothing.
        let sent = self.pending.iter().find(|(sent, _)| *sent == id)?.1;
        let delay = now.saturating_duration_since(sent);
        paneferry_backend_windows::metrics::sample("publisher.feedback_roundtrip", delay);
        while self.pending.front().is_some_and(|(sent, _)| *sent <= id) {
            self.pending.pop_front();
        }
        let baseline = self.baseline.map_or(delay, |old| old.min(delay));
        self.baseline = Some(baseline);
        let excess = delay.saturating_sub(baseline);
        let cooldown = self
            .last_adjusted
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(1));
        let previous = self.quality;
        if excess > Duration::from_millis(80) {
            self.stable_since = None;
            self.bad_samples = self.bad_samples.saturating_add(1);
            if self.bad_samples >= 3 && cooldown {
                self.quality = (self.quality * 4 / 5).max(250);
                self.bad_samples = 0;
            }
        } else {
            self.bad_samples = 0;
            let since = *self.stable_since.get_or_insert(now);
            if cooldown && now.saturating_duration_since(since) >= Duration::from_secs(5) {
                self.quality = (self.quality + 100).min(1000);
                self.stable_since = Some(now);
            }
        }
        if self.quality != previous {
            self.last_adjusted = Some(now);
            eprintln!(
                "PF_VIDEO quality_per_mille={} feedback_excess_ms={}",
                self.quality,
                excess.as_millis()
            );
            Some(self.quality)
        } else {
            None
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
    fn adaptation_ignores_jitter_and_unknown_acks_then_recovers() {
        let now = Instant::now();
        let mut flow = VideoFlow::default();
        flow.enable(true);
        let id = flow.sent(now).unwrap();
        assert_eq!(
            flow.acknowledge_at(id, now + Duration::from_millis(30)),
            None
        );
        assert_eq!(flow.acknowledge_at(999, now + Duration::from_secs(9)), None);
        for index in 1..=3 {
            let sent = now + Duration::from_secs(index);
            let id = flow.sent(sent).unwrap();
            assert_eq!(
                flow.acknowledge_at(id, sent + Duration::from_millis(160)),
                if index == 3 { Some(800) } else { None }
            );
        }
        let sent = now + Duration::from_secs(4);
        let id = flow.sent(sent).unwrap();
        assert_eq!(
            flow.acknowledge_at(id, sent + Duration::from_millis(35)),
            None
        );
        let sent = now + Duration::from_secs(10);
        let id = flow.sent(sent).unwrap();
        assert_eq!(
            flow.acknowledge_at(id, sent + Duration::from_millis(35)),
            Some(900)
        );
        assert_eq!(flow.acknowledge_at(id, sent + Duration::from_secs(2)), None);
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
