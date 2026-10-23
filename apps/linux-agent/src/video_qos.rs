//! Per-controller video pacing. Feedback is an acknowledgement of a frame
//! actually presented by the client, so it includes encoding and queue delay.
use paneferry_protocol::DEFAULT_VIDEO_FPS;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const MIN_FPS: u8 = 15;
const HISTORY: usize = 24;
const MAX_PENDING: usize = 128;

pub struct VideoQos {
    cap: u8,
    target: u8,
    feedback: bool,
    next_id: u64,
    pending: VecDeque<(u64, Instant)>,
    delays: VecDeque<Duration>,
    good: u8,
    bad: u8,
    last_change: Instant,
    last_feedback: Instant,
}

impl VideoQos {
    pub fn new(now: Instant) -> Self {
        Self {
            cap: DEFAULT_VIDEO_FPS,
            target: DEFAULT_VIDEO_FPS,
            feedback: false,
            next_id: 1,
            pending: VecDeque::new(),
            delays: VecDeque::new(),
            good: 0,
            bad: 0,
            last_change: now,
            last_feedback: now,
        }
    }

    pub fn configure(&mut self, cap: u8, feedback: bool, now: Instant) {
        self.cap = cap;
        self.target = cap;
        self.feedback = feedback;
        self.pending.clear();
        self.delays.clear();
        self.good = 0;
        self.bad = 0;
        self.last_change = now;
        self.last_feedback = now;
    }

    pub fn cap(&self) -> u8 {
        self.cap
    }
    pub fn feedback_enabled(&self) -> bool {
        self.feedback
    }
    pub fn target(&self) -> u8 {
        self.target
    }
    pub fn interval(&mut self, now: Instant) -> Duration {
        self.check_stall(now);
        Duration::from_micros(1_000_000 / u64::from(self.target))
    }

    pub fn sent(&mut self, now: Instant) -> Option<u64> {
        if !self.feedback {
            return None;
        }
        self.check_stall(now);
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back((id, now));
        Some(id)
    }

    pub fn acknowledge(&mut self, id: u64, now: Instant) {
        if !self.feedback {
            return;
        }
        let Some(position) = self.pending.iter().position(|(sent, _)| *sent == id) else {
            return;
        };
        self.last_feedback = now;
        let delay = now.saturating_duration_since(self.pending[position].1);
        for _ in 0..=position {
            self.pending.pop_front();
        }
        self.delays.push_back(delay);
        if self.delays.len() > HISTORY {
            self.delays.pop_front();
        }
        let baseline = *self.delays.iter().min().unwrap_or(&delay);
        let excess = delay.saturating_sub(baseline);
        if excess > Duration::from_millis(100) || self.pending.len() > 4 {
            self.good = 0;
            self.bad = self.bad.saturating_add(1);
            if self.bad >= 2
                && now.saturating_duration_since(self.last_change) >= Duration::from_millis(500)
            {
                self.target =
                    ((u16::from(self.target) * 3) / 4).max(u16::from(MIN_FPS.min(self.cap))) as u8;
                self.last_change = now;
                self.bad = 0;
            }
        } else if excess < Duration::from_millis(35) && self.pending.len() <= 2 {
            self.bad = 0;
            self.good = self.good.saturating_add(1);
            if self.good >= 4
                && now.saturating_duration_since(self.last_change) >= Duration::from_millis(500)
            {
                self.target = self.target.saturating_add(5).min(self.cap);
                self.last_change = now;
                self.good = 0;
            }
        } else {
            self.good = 0;
            self.bad = 0;
        }
    }

    fn check_stall(&mut self, now: Instant) {
        if !self.feedback
            || self.pending.is_empty()
            || now.saturating_duration_since(self.last_feedback) < Duration::from_secs(1)
            || now.saturating_duration_since(self.last_change) < Duration::from_secs(1)
        {
            return;
        }
        self.target = (self.target / 2).max(MIN_FPS.min(self.cap));
        self.last_change = now;
        self.good = 0;
        self.bad = 0;
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn congestion_backs_off_and_clean_feedback_recovers_without_exceeding_cap() {
        let start = Instant::now();
        let mut qos = VideoQos::new(start);
        qos.configure(90, true, start);
        for i in 0..2 {
            let sent = start + Duration::from_millis(600 + i * 600);
            let id = qos.sent(sent).unwrap();
            qos.acknowledge(
                id,
                sent + Duration::from_millis(if i == 0 { 20 } else { 180 }),
            );
        }
        let sent = start + Duration::from_millis(1800);
        let id = qos.sent(sent).unwrap();
        qos.acknowledge(id, sent + Duration::from_millis(180));
        assert!(qos.target() < 90);
        for i in 0..32 {
            let sent = start + Duration::from_millis(2500 + i * 300);
            let id = qos.sent(sent).unwrap();
            qos.acknowledge(id, sent + Duration::from_millis(20));
        }
        assert_eq!(qos.target(), 90);
        qos.configure(60, true, start + Duration::from_secs(20));
        assert_eq!(qos.target(), 60);
    }
    #[test]
    fn missing_feedback_cuts_rate_without_stalling_legacy_clients() {
        let start = Instant::now();
        let mut qos = VideoQos::new(start);
        assert_eq!(
            qos.interval(start + Duration::from_secs(2)),
            Duration::from_micros(16_666)
        );
        qos.configure(60, true, start);
        qos.sent(start);
        qos.interval(start + Duration::from_secs(2));
        assert_eq!(qos.target(), 30);
        qos.sent(start + Duration::from_secs(2));
        qos.interval(start + Duration::from_secs(4));
        assert_eq!(qos.target(), MIN_FPS);
    }

    #[test]
    fn rolling_pending_limit_does_not_hide_a_stalled_client() {
        let start = Instant::now();
        let mut qos = VideoQos::new(start);
        qos.configure(90, true, start);
        for index in 0..200 {
            qos.sent(start + Duration::from_millis(index * 5));
        }
        qos.interval(start + Duration::from_secs(2));
        assert_eq!(qos.target(), 45);
    }
}
