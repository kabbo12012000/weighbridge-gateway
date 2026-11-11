//! Derives stability for indicators that do not transmit a stable/motion flag:
//! a reading is stable once the weight has stayed within `tolerance` for `window`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct MotionDetector {
    window: Duration,
    tolerance: f64,
    samples: VecDeque<(Instant, f64)>,
}

impl MotionDetector {
    pub fn new(window_ms: u64, tolerance: f64) -> Self {
        Self { window: Duration::from_millis(window_ms), tolerance, samples: VecDeque::new() }
    }

    pub fn push(&mut self, now: Instant, weight: f64) -> bool {
        self.samples.push_back((now, weight));
        // Keep one sample older than the window so we can tell the window is fully covered.
        while self.samples.len() > 2 && now.duration_since(self.samples[1].0) >= self.window {
            self.samples.pop_front();
        }
        let oldest = self.samples.front().map(|s| s.0).unwrap_or(now);
        if now.duration_since(oldest) < self.window {
            return false;
        }
        let (min, max) = self
            .samples
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &(_, w)| (lo.min(w), hi.max(w)));
        max - min <= self.tolerance
    }

    pub fn reset(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn becomes_stable_after_window() {
        let mut m = MotionDetector::new(1000, 0.0);
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert!(!m.push(ms(0), 100.0));
        assert!(!m.push(ms(500), 100.0));
        assert!(m.push(ms(1000), 100.0));
        assert!(!m.push(ms(1200), 120.0)); // moved
        assert!(!m.push(ms(1800), 120.0));
        assert!(m.push(ms(2300), 120.0));
    }
}
