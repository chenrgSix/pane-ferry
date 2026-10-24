//! Fit an oversized client to the frame budget without fighting its WM hints.
use std::time::{Duration, Instant};
use x11rb::properties::WmSizeHints;

pub(super) fn fit(
    current: (u32, u32),
    bounds: (u32, u32),
    hints: &WmSizeHints,
) -> Option<(u32, u32)> {
    let minimum = hints.min_size.or(hints.base_size).unwrap_or((1, 1));
    let base = hints.base_size.or(hints.min_size).unwrap_or((0, 0));
    let maximum = hints.max_size.unwrap_or((4096, 4096));
    let increment = hints.size_increment.unwrap_or((1, 1));
    // Malformed hints are not a reason to remove a client's constraints.
    if minimum.0 < 0
        || minimum.1 < 0
        || base.0 < 0
        || base.1 < 0
        || maximum.0 <= 0
        || maximum.1 <= 0
        || increment.0 <= 0
        || increment.1 <= 0
    {
        return None;
    }
    let lo = (minimum.0.max(1) as u32, minimum.1.max(1) as u32);
    let hi = (
        (maximum.0 as u32).min(bounds.0).min(4096),
        (maximum.1 as u32).min(bounds.1).min(4096),
    );
    let base = (base.0 as u32, base.1 as u32);
    let inc = (increment.0 as u32, increment.1 as u32);
    if lo.0 > hi.0 || lo.1 > hi.1 {
        return None;
    }
    let aspect_base = hints.base_size.unwrap_or((0, 0));
    if let Some((min, max)) = hints.aspect {
        if min.numerator <= 0
            || min.denominator <= 0
            || max.numerator <= 0
            || max.denominator <= 0
            || i64::from(min.numerator) * i64::from(max.denominator)
                > i64::from(max.numerator) * i64::from(min.denominator)
        {
            return None;
        }
    }
    let budget = paneferry_protocol::MAX_PIXELS;
    let factor = (budget as f64 / (f64::from(current.0.max(1)) * f64::from(current.1.max(1))))
        .sqrt()
        .min(1.0)
        .min(f64::from(hi.0) / f64::from(current.0.max(1)))
        .min(f64::from(hi.1) / f64::from(current.1.max(1)));
    let ideal = (
        f64::from(current.0.max(1)) * factor,
        f64::from(current.1.max(1)) * factor,
    );
    let mut best = None;
    let mut score = f64::INFINITY;
    // At most 4096 widths, with only two candidate heights per width. The
    // budget and aspect limits give an interval before increment alignment.
    for w in lo.0..=hi.0 {
        if w < base.0 || (w - base.0) % inc.0 != 0 {
            continue;
        }
        let mut low = lo.1.max(base.1);
        let mut high = hi.1.min((budget / u64::from(w)) as u32);
        if let Some((min, max)) = hints.aspect {
            let aw = i64::from(w) - i64::from(aspect_base.0);
            if aw <= 0 {
                continue;
            }
            let lower = i64::from(aspect_base.1)
                + (aw * i64::from(max.denominator) + i64::from(max.numerator) - 1)
                    / i64::from(max.numerator);
            let upper = i64::from(aspect_base.1)
                + aw * i64::from(min.denominator) / i64::from(min.numerator);
            if lower > i64::from(high) || upper < i64::from(low) {
                continue;
            }
            low = low.max(lower.max(1) as u32);
            high = high.min(upper.min(i64::from(high)) as u32);
        }
        if low > high {
            continue;
        }
        let first = u64::from(base.1)
            + u64::from(low - base.1).div_ceil(u64::from(inc.1)) * u64::from(inc.1);
        if first > u64::from(high) {
            continue;
        }
        let first = first as u32;
        let last = base.1 + ((high - base.1) / inc.1) * inc.1;
        let near = (ideal.1.round() as u32).clamp(first, last);
        let below = base.1 + ((near - base.1) / inc.1) * inc.1;
        for h in [below.max(first), below.saturating_add(inc.1).min(last)] {
            let distance =
                (f64::from(w) / ideal.0 - 1.0).powi(2) + (f64::from(h) / ideal.1 - 1.0).powi(2);
            if distance < score {
                score = distance;
                best = Some((w, h));
            }
        }
    }
    best
}

pub(super) struct Adjustment {
    started: Instant,
    sent: Instant,
    target: (u32, u32),
}
impl Adjustment {
    pub fn new(target: (u32, u32), now: Instant) -> Self {
        Self {
            started: now,
            sent: now,
            target,
        }
    }
    pub fn update(&mut self, target: (u32, u32), now: Instant) -> Result<bool, &'static str> {
        // A changing size/hint cannot reset the overall deadline forever.
        if now.duration_since(self.started) >= Duration::from_secs(2) {
            return Err("window_resize_failed");
        }
        let delay = if self.target == target { 500 } else { 250 };
        if now.duration_since(self.sent) < Duration::from_millis(delay) {
            return Ok(false);
        }
        self.target = target;
        self.sent = now;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::properties::AspectRatio;
    #[test]
    fn respects_tall_minimum_instead_of_requesting_an_impossible_height() {
        let hints = WmSizeHints {
            min_size: Some((1920, 2400)),
            ..Default::default()
        };
        let size = fit((3703, 2400), (4096, 4096), &hints).unwrap();
        assert_eq!(size, (3333, 2400));
        assert!(paneferry_protocol::dimensions(size.0, size.1).is_ok());
        assert_eq!(fit((3703, 2400), (4096, 2160), &hints), None);
    }
    #[test]
    fn respects_base_increment_aspect_and_maximum_together() {
        let hints = WmSizeHints {
            min_size: Some((320, 240)),
            max_size: Some((3500, 3000)),
            base_size: Some((20, 40)),
            size_increment: Some((8, 16)),
            aspect: Some((AspectRatio::new(4, 3), AspectRatio::new(16, 9))),
            ..Default::default()
        };
        let (w, h) = fit((6000, 5000), (4096, 4096), &hints).unwrap();
        assert!((320..=3500).contains(&w) && (240..=3000).contains(&h));
        assert_eq!((w - 20) % 8, 0);
        assert_eq!((h - 40) % 16, 0);
        assert!((w - 20) * 3 >= (h - 40) * 4 && (w - 20) * 9 <= (h - 40) * 16);
        assert!(paneferry_protocol::dimensions(w, h).is_ok());
    }
    #[test]
    fn does_not_subtract_minimum_from_aspect_without_explicit_base() {
        let hints = WmSizeHints {
            min_size: Some((1000, 500)),
            aspect: Some((AspectRatio::new(1, 1), AspectRatio::new(1, 1))),
            ..Default::default()
        };
        let (w, h) = fit((6000, 5000), (4096, 4096), &hints).unwrap();
        assert_eq!(w, h);
    }
    #[test]
    fn rejects_impossible_or_malformed_constraints() {
        for hints in [
            WmSizeHints {
                min_size: Some((3000, 3000)),
                ..Default::default()
            },
            WmSizeHints {
                min_size: Some((900, 900)),
                max_size: Some((500, 500)),
                ..Default::default()
            },
            WmSizeHints {
                size_increment: Some((0, 1)),
                ..Default::default()
            },
            WmSizeHints {
                base_size: Some((i32::MAX, i32::MAX)),
                ..Default::default()
            },
        ] {
            assert!(fit((5000, 5000), (4096, 4096), &hints).is_none());
        }
    }
    #[test]
    fn throttles_resizes_and_bounds_total_wait_even_if_target_changes() {
        let now = Instant::now();
        let mut request = Adjustment::new((3000, 2400), now);
        assert_eq!(
            request.update((3100, 2400), now + Duration::from_millis(10)),
            Ok(false)
        );
        assert_eq!(
            request.update((3100, 2400), now + Duration::from_millis(250)),
            Ok(true)
        );
        assert_eq!(
            request.update((3200, 2400), now + Duration::from_secs(2)),
            Err("window_resize_failed")
        );
    }
}
