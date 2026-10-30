//! Opt-in, bounded timing aggregates. Never record pixels, input values or titles.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::OnceLock,
    time::{Duration, Instant},
};

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("PANEFERRY_VIDEO_TIMING").as_deref() == Ok("1"))
}
struct Samples {
    start: Instant,
    values: BTreeMap<&'static str, Vec<u64>>,
}
thread_local! {
    static SAMPLES: RefCell<Samples> = RefCell::new(Samples { start: Instant::now(), values: BTreeMap::new() });
}
pub fn sample(stage: &'static str, elapsed: Duration) {
    if !enabled() {
        return;
    }
    SAMPLES.with(|samples| {
        let mut samples = samples.borrow_mut();
        let values = samples.values.entry(stage).or_default();
        // Bound memory even when several hundred input events arrive per second.
        if values.len() < 2048 { values.push(elapsed.as_micros().min(u64::MAX as u128) as u64); }
        if samples.start.elapsed() < Duration::from_secs(2) { return; }
        let interval = samples.start.elapsed().as_millis();
        for (stage, values) in &mut samples.values {
            if values.is_empty() { continue; }
            values.sort_unstable();
            eprintln!("PF_TIMING stage={stage} interval_ms={interval} count={} p50_us={} p95_us={} p99_us={}",
                values.len(), percentile(values, 50), percentile(values, 95), percentile(values, 99));
            values.clear();
        }
        samples.start = Instant::now();
    });
}
fn percentile(sorted: &[u64], percent: usize) -> u64 {
    sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)]
}

#[cfg(test)]
mod tests {
    use super::percentile;
    #[test]
    fn nearest_rank_percentiles_remain_ordered_for_small_samples() {
        for len in 1..=2048 {
            let values: Vec<_> = (0..len).collect();
            let p50 = percentile(&values, 50);
            let p95 = percentile(&values, 95);
            let p99 = percentile(&values, 99);
            assert!(p50 <= p95 && p95 <= p99);
        }
        assert_eq!(percentile(&[10, 20], 95), 20);
        assert_eq!(percentile(&[10, 20], 50), 10);
    }
}
pub struct Span {
    stage: &'static str,
    start: Option<Instant>,
}
pub fn span(stage: &'static str) -> Span {
    Span {
        stage,
        start: enabled().then(Instant::now),
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            sample(self.stage, start.elapsed());
        }
    }
}
