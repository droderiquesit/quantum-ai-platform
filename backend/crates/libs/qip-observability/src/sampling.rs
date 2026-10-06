//! Strict sampling for log lines on high-rate paths.
//!
//! A per-event line on a path that runs at message rate makes log volume
//! proportional to traffic, and on a deployed service stdout is billed log
//! ingestion. The count of what was dropped is carried on the next line that
//! gets through, and the event itself belongs in a counter, so sampling loses
//! volume and never the fact that something happened.

use qip_core::{Duration, Error, Result, Timestamp};

/// At most `per_window` lines per `window`, however many are offered.
#[derive(Debug)]
pub struct LineSampler {
    per_window: u32,
    window: Duration,
    window_start: Option<Timestamp>,
    emitted: u32,
    suppressed: u64,
}

impl LineSampler {
    /// A zero rate or a zero-length window is refused: the first would drop
    /// everything silently and the second would never bound anything.
    pub fn new(per_window: u32, window: Duration) -> Result<Self> {
        if per_window == 0 || window.is_zero() {
            return Err(Error::invalid(
                "a log sampler needs at least one line per window and a non-zero window; set a positive rate",
            ));
        }
        Ok(Self {
            per_window,
            window,
            window_start: None,
            emitted: 0,
            suppressed: 0,
        })
    }

    /// The line to write, or `None` when the window's allowance is spent. The
    /// first line of a new window reports how many the last window dropped.
    pub fn offer(&mut self, now: Timestamp, line: &str) -> Option<String> {
        if self
            .window_start
            .is_none_or(|start| now.since(start) >= self.window)
        {
            self.window_start = Some(now);
            self.emitted = 0;
        }
        if self.emitted >= self.per_window {
            self.suppressed += 1;
            return None;
        }
        self.emitted += 1;
        if self.suppressed > 0 {
            let dropped = std::mem::take(&mut self.suppressed);
            return Some(format!("{line} ({dropped} similar line(s) sampled out)"));
        }
        Some(line.to_string())
    }
}
