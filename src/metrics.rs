#[cfg(feature = "metrics")]
use metrics::histogram;

#[cfg(feature = "metrics")]
pub(crate) struct TimerGuard {
    start: std::time::Instant,
    name: &'static str,
    event: &'static str,
    handler: Option<&'static str>,
}

#[cfg(feature = "metrics")]
impl TimerGuard {
    pub fn start(name: &'static str, event: &'static str, handler: Option<&'static str>) -> Self {
        Self {
            start: std::time::Instant::now(),
            name,
            event,
            handler,
        }
    }
}

#[cfg(feature = "metrics")]
impl Drop for TimerGuard {
    fn drop(&mut self) {
        let dur = self.start.elapsed();
        let histogram = if let Some(listener) = self.handler {
            histogram!(self.name, "event" => self.event, "handler" => listener)
        } else {
            histogram!(self.name, "event" => self.event)
        };
        histogram.record(dur.as_secs_f64());
    }
}
