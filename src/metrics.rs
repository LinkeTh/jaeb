// SPDX-License-Identifier: MIT
#[cfg(feature = "metrics")]
use metrics::histogram;

#[cfg(feature = "metrics")]
pub(crate) struct TimerGuard {
    start: std::time::Instant,
    name: &'static str,
    event: &'static str,
}

#[cfg(feature = "metrics")]
impl TimerGuard {
    pub fn start(name: &'static str, event: &'static str) -> Self {
        Self {
            start: std::time::Instant::now(),
            name,
            event,
        }
    }
}

#[cfg(feature = "metrics")]
impl Drop for TimerGuard {
    fn drop(&mut self) {
        let dur = self.start.elapsed();
        let histogram = histogram!(self.name, "event" => self.event);
        histogram.record(dur.as_secs_f64());
    }
}
