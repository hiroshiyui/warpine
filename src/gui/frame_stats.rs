// SPDX-License-Identifier: GPL-3.0-only

//! Timing statistics for the PM GUI loop.
//!
//! The renderer records every loop iteration and every composite; every
//! [`REPORT_INTERVAL`] it logs one summary line at debug level (enable with
//! `RUST_LOG=warpine::gui=debug`). The numbers are what the graphics
//! performance work in `doc/TODOs.md` is measured against.

use std::time::{Duration, Instant};

/// How often a summary is produced.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Counters for one reporting interval.
pub struct FrameStats {
    window_start: Instant,
    iterations: u32,
    composites: u32,
    composite_total: Duration,
    composite_max: Duration,
    present_total: Duration,
    /// Sum over composites of the redrawn fraction of the desktop.
    damaged_total: f64,
}

impl FrameStats {
    pub fn new(now: Instant) -> Self {
        FrameStats {
            window_start: now,
            iterations: 0,
            composites: 0,
            composite_total: Duration::ZERO,
            composite_max: Duration::ZERO,
            present_total: Duration::ZERO,
            damaged_total: 0.0,
        }
    }

    /// One pass of the GUI loop (drain messages, poll input, wait).
    pub fn record_iteration(&mut self) {
        self.iterations += 1;
    }

    /// One composite: `total` covers the whole composite including
    /// `present`, the upload of the desktop buffer and the present itself;
    /// `damaged` is the fraction of the desktop redrawn (0.0–1.0).
    pub fn record_composite(&mut self, total: Duration, present: Duration, damaged: f64) {
        self.composites += 1;
        self.damaged_total += damaged;
        self.composite_total += total;
        self.composite_max = self.composite_max.max(total);
        self.present_total += present;
    }

    /// The summary for the interval that ended at `now`, if it has ended;
    /// the counters then start a new interval.
    pub fn take_report(&mut self, now: Instant) -> Option<String> {
        let elapsed = now.saturating_duration_since(self.window_start);
        if elapsed < REPORT_INTERVAL {
            return None;
        }
        let secs = elapsed.as_secs_f64();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let avg = |d: Duration| if self.composites == 0 { 0.0 } else { ms(d) / self.composites as f64 };
        let report = format!(
            "{secs:.1} s: {} loop iterations ({:.0}/s), {} composites ({:.0}/s), \
             composite avg {:.2} ms max {:.2} ms, of which present avg {:.2} ms; \
             avg {:.0}% of the desktop redrawn",
            self.iterations, self.iterations as f64 / secs,
            self.composites, self.composites as f64 / secs,
            avg(self.composite_total), ms(self.composite_max), avg(self.present_total),
            if self.composites == 0 { 0.0 } else { 100.0 * self.damaged_total / self.composites as f64 });
        *self = FrameStats::new(now);
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_report_before_the_interval_ends() {
        let t0 = Instant::now();
        let mut stats = FrameStats::new(t0);
        stats.record_iteration();
        assert!(stats.take_report(t0 + REPORT_INTERVAL / 2).is_none());
    }

    #[test]
    fn report_summarises_the_interval_and_resets() {
        let t0 = Instant::now();
        let mut stats = FrameStats::new(t0);
        for _ in 0..10 { stats.record_iteration(); }
        stats.record_composite(Duration::from_millis(2), Duration::from_millis(1), 1.0);
        stats.record_composite(Duration::from_millis(4), Duration::from_millis(1), 0.5);
        let report = stats.take_report(t0 + REPORT_INTERVAL).expect("interval has ended");
        assert!(report.contains("10 loop iterations (2/s)"), "{report}");
        assert!(report.contains("2 composites"), "{report}");
        assert!(report.contains("composite avg 3.00 ms max 4.00 ms"), "{report}");
        assert!(report.contains("present avg 1.00 ms"), "{report}");
        assert!(report.contains("avg 75% of the desktop redrawn"), "{report}");

        // A new interval starts empty.
        let t1 = t0 + REPORT_INTERVAL * 2;
        let report = stats.take_report(t1).expect("second interval has ended");
        assert!(report.contains(" 0 loop iterations") && report.contains(" 0 composites"), "{report}");
        assert!(report.contains("composite avg 0.00 ms"), "{report}");
    }
}
