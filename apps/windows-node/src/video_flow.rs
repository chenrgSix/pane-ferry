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
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackTiming {
    pub queue_us: u32,
    pub decode_us: u32,
    pub present_us: u32,
    pub feedback_wait_us: u32,
}
impl FeedbackTiming {
    pub fn valid(&self) -> bool {
        [
            self.queue_us,
            self.decode_us,
            self.present_us,
            self.feedback_wait_us,
        ]
        .into_iter()
        .all(|us| us <= 5_000_000)
    }
    fn record(&self, roundtrip: Duration) {
        let total = Duration::from_micros(
            self.queue_us as u64
                + self.decode_us as u64
                + self.present_us as u64
                + self.feedback_wait_us as u64,
        );
        // These are optional, untrusted peer measurements, never flow-control
        // credits. Ignore inconsistent reports and keep the real local RTT.
        if !self.valid() || total > roundtrip {
            return;
        }
        for (stage, us) in [
            ("receiver.decode_queue", self.queue_us),
            ("receiver.decode", self.decode_us),
            ("receiver.decode_to_commit", self.present_us),
            ("receiver.feedback_hold", self.feedback_wait_us),
        ] {
            paneferry_backend_windows::metrics::sample(stage, Duration::from_micros(us as u64));
        }
        // Includes transport, IPC/bridge work and the feedback return path.
        // This residual is deliberately not labelled as pure network latency.
        paneferry_backend_windows::metrics::sample(
            "publisher.transport_and_bridge_roundtrip",
            roundtrip - total,
        );
    }
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
    gate_started: Option<Instant>,
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
        self.gate_started = None;
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
            self.finish_gate(now);
            return Ready::Restart;
        }
        // Allow the Mac host's 50 ms feedback cadence plus ~30 ms RTT at 60 fps.
        // Bound stalled display/network queues without throttling healthy feedback.
        if self.pending.len() >= 6 {
            self.gate_started.get_or_insert(now);
            Ready::Wait
        } else {
            self.finish_gate(now);
            Ready::Send
        }
    }
    fn finish_gate(&mut self, now: Instant) {
        if let Some(started) = self.gate_started.take() {
            paneferry_backend_windows::metrics::sample(
                "publisher.feedback_gate_wait",
                now.saturating_duration_since(started),
            );
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
    pub fn acknowledge(&mut self, id: u64, timing: Option<FeedbackTiming>) -> Option<u32> {
        let now = Instant::now();
        if let (Some(timing), Some((_, sent))) =
            (timing, self.pending.iter().find(|(frame, _)| *frame == id))
        {
            timing.record(now.saturating_duration_since(*sent));
        }
        self.acknowledge_at(id, now)
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
    fn reported_receiver_timing_cannot_add_credits_for_unknown_frames() {
        let now = Instant::now();
        let mut flow = VideoFlow::default();
        flow.enable(true);
        for _ in 0..6 {
            flow.sent(now);
        }
        assert_eq!(flow.ready(now), Ready::Wait);
        assert!(flow.gate_started.is_some());
        let sample = || {
            Some(FeedbackTiming {
                queue_us: 1,
                decode_us: 1,
                present_us: 1,
                feedback_wait_us: 1,
            })
        };
        flow.acknowledge(999, sample());
        assert_eq!(flow.ready(now + Duration::from_millis(100)), Ready::Wait);
        flow.acknowledge(3, sample());
        assert_eq!(flow.ready(now + Duration::from_millis(101)), Ready::Send);
        assert!(flow.gate_started.is_none());
        assert_eq!(flow.pending.len(), 3);
    }
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
        flow.acknowledge(999, None);
        assert_eq!(flow.ready(now), Ready::Wait);
        flow.acknowledge(3, None);
        assert_eq!(flow.ready(now), Ready::Send);
        flow.sent(now);
        assert_eq!(flow.ready(now + Duration::from_millis(501)), Ready::Restart);
        assert_eq!(flow.ready(now + Duration::from_millis(502)), Ready::Send);
        assert_eq!(flow.sent(now), Some(8));
        flow.acknowledge(4, None);
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
