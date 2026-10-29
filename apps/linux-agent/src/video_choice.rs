//! Compare whole capture-to-encode service time on actual published frames.
//! Each frame is encoded once; codec changes start fresh streams/IDRs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    CpuProbe,
    GpuProbe,
    Gpu,
    Cpu,
    Fixed,
}
pub(super) struct Choice {
    phase: Phase,
    seen: usize,
    samples: Vec<(f64, f64)>,
    baseline: f64,
    service_baseline: f64,
}
impl Choice {
    pub fn new(probe: bool) -> Self {
        Self {
            phase: if probe { Phase::CpuProbe } else { Phase::Fixed },
            seen: 0,
            samples: Vec::new(),
            baseline: 0.0,
            service_baseline: 0.0,
        }
    }
    pub fn starts_software(&self) -> bool {
        self.phase == Phase::CpuProbe
    }
    pub fn attempted_gpu(&self) -> bool {
        matches!(self.phase, Phase::GpuProbe | Phase::Gpu | Phase::Fixed)
    }
    pub fn software_selected(&self) -> bool {
        self.phase == Phase::Cpu
    }
    /// Some(true/false) restarts the next frame on GPU/software respectively.
    pub fn observe(&mut self, ms: f64, service: f64, gpu: bool) -> Option<bool> {
        if matches!(self.phase, Phase::Fixed | Phase::Cpu) {
            return None;
        }
        if self.phase != Phase::CpuProbe && !gpu {
            self.phase = Phase::Cpu;
            return None;
        }
        self.seen += 1;
        // Exclude each new encoder's four cold frames. No extra encodes or
        // CPU pixel reads are inserted into GPU samples.
        if self.seen > 4 && ms.is_finite() && ms > 0.0 && service.is_finite() && service > 0.0 {
            self.samples.push((ms, service));
        }
        if self.samples.len() < 12 {
            return None;
        }
        self.samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        let median = (self.samples[5].0 + self.samples[6].0) / 2.0;
        self.samples.sort_by(|a, b| a.1.total_cmp(&b.1));
        let service_median = (self.samples[5].1 + self.samples[6].1) / 2.0;
        self.samples.clear();
        self.seen = 0;
        match self.phase {
            Phase::CpuProbe => {
                self.baseline = median;
                self.service_baseline = service_median;
                self.phase = Phase::GpuProbe;
                Some(true)
            }
            Phase::GpuProbe => {
                let keep =
                    median < self.baseline * 0.92 && service_median <= self.service_baseline * 1.03;
                eprintln!(
                    "video path comparison: CPU {:.1} ms, GPU {:.1} ms, bottleneck {:.1}/{:.1} ms; {}",
                    self.baseline,
                    median,
                    self.service_baseline,
                    service_median,
                    if keep { "GPU" } else { "software" }
                );
                self.phase = if keep { Phase::Gpu } else { Phase::Cpu };
                if keep {
                    self.seen = 4;
                }
                (!keep).then_some(false)
            }
            Phase::Gpu
                if median > self.baseline * 1.15
                    || service_median > self.service_baseline * 1.10 =>
            {
                self.phase = Phase::Cpu;
                eprintln!("video path: GPU service time increased; restarting software");
                Some(false)
            }
            Phase::Gpu => {
                self.seen = 4;
                None
            }
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frames(choice: &mut Choice, ms: f64, gpu: bool) -> Option<bool> {
        (0..16).filter_map(|_| choice.observe(ms, ms, gpu)).last()
    }
    #[test]
    fn faster_gpu_is_kept_but_later_contention_returns_to_software() {
        let mut c = Choice::new(true);
        assert!(c.starts_software());
        assert_eq!(frames(&mut c, 40.0, false), Some(true));
        assert_eq!(frames(&mut c, 30.0, true), None);
        assert_eq!(c.phase, Phase::Gpu);
        assert_eq!(frames(&mut c, 50.0, true), Some(false));
        assert_eq!(frames(&mut c, 20.0, false), None);
    }
    #[test]
    fn small_or_negative_gain_returns_to_software_and_failures_do_not_loop() {
        for gpu_ms in [38.0, 45.0] {
            let mut c = Choice::new(true);
            assert_eq!(frames(&mut c, 40.0, false), Some(true));
            assert_eq!(frames(&mut c, gpu_ms, true), Some(false));
            assert!(!c.attempted_gpu());
        }
        let mut c = Choice::new(true);
        frames(&mut c, 40.0, false);
        assert_eq!(c.observe(10.0, 10.0, false), None);
        assert_eq!(c.phase, Phase::Cpu);
        assert_eq!(frames(&mut Choice::new(false), 100.0, true), None);
    }
    #[test]
    fn latency_gain_does_not_justify_a_slower_capture_bottleneck() {
        let mut c = Choice::new(true);
        assert_eq!(frames(&mut c, 40.0, false), Some(true));
        assert_eq!(
            (0..16).filter_map(|_| c.observe(30.0, 50.0, true)).last(),
            Some(false)
        );
        assert_eq!(c.phase, Phase::Cpu);
    }
}
