use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::AbortHandle;

pub(crate) struct AsyncTaskTracker {
    next_id: AtomicU64,
    in_flight: AtomicUsize,
    tasks: Option<StdMutex<HashMap<u64, AbortHandle>>>,
    notify: Notify,
}

impl Default for AsyncTaskTracker {
    fn default() -> Self {
        Self::new(true)
    }
}

impl AsyncTaskTracker {
    pub(crate) fn new(track_abort_handles: bool) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            in_flight: AtomicUsize::new(0),
            tasks: track_abort_handles.then(|| StdMutex::new(HashMap::new())),
            notify: Notify::new(),
        }
    }

    pub(crate) fn spawn_tracked<F>(self: &Arc<Self>, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        let tracker = Arc::clone(self);

        if let Some(tasks) = &self.tasks {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let mut guard = tasks.lock().expect("tracker task lock poisoned");

            // Keep the map lock across spawn so the abort handle is inserted
            // before the task can complete and attempt removal.
            let handle = tokio::spawn(async move {
                fut.await;
                tracker.finish_task(Some(id));
            });

            guard.insert(id, handle.abort_handle());
        } else {
            tokio::spawn(async move {
                fut.await;
                tracker.finish_task(None);
            });
        }
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    pub(crate) async fn shutdown(&self, timeout: Option<Duration>) -> bool {
        if self.in_flight() == 0 {
            return false;
        }

        if let Some(timeout) = timeout {
            let wait = async {
                loop {
                    let notified = self.notify.notified();
                    if self.in_flight() == 0 {
                        return;
                    }
                    notified.await;
                }
            };
            if tokio::time::timeout(timeout, wait).await.is_err() {
                debug_assert!(
                    self.tasks.is_some(),
                    "shutdown timeout requires tracked abort handles to cancel in-flight tasks"
                );
                let handles: Vec<AbortHandle> = self
                    .tasks
                    .as_ref()
                    .map(|tasks| {
                        let mut guard = tasks.lock().expect("tracker task lock poisoned");
                        guard.drain().map(|(_, h)| h).collect()
                    })
                    .unwrap_or_default();
                for handle in &handles {
                    handle.abort();
                }
                return true;
            }
            false
        } else {
            loop {
                let notified = self.notify.notified();
                if self.in_flight() == 0 {
                    break;
                }
                notified.await;
            }
            false
        }
    }

    fn finish_task(&self, id: Option<u64>) {
        let prev = self.in_flight.fetch_sub(1, Ordering::AcqRel);
        if let Some(id) = id {
            self.remove_abort_handle(id);
        }
        if prev == 1 {
            self.notify.notify_waiters();
        }
    }

    fn remove_abort_handle(&self, id: u64) {
        if let Some(tasks) = &self.tasks {
            tasks.lock().expect("tracker task lock poisoned").remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AsyncTaskTracker;
    use std::sync::Arc;
    use tokio::sync::Barrier;

    #[tokio::test]
    async fn tracker_does_not_leak_handles_for_fast_tasks() {
        let tracker = Arc::new(AsyncTaskTracker::new(true));
        let barrier = Arc::new(Barrier::new(2));

        tracker.spawn_tracked({
            let barrier = Arc::clone(&barrier);
            async move {
                barrier.wait().await;
            }
        });

        barrier.wait().await;
        let timed_out = tracker.shutdown(Some(std::time::Duration::from_secs(1))).await;
        assert!(!timed_out, "tracker shutdown should complete without timeout");

        let guard = tracker
            .tasks
            .as_ref()
            .expect("tracker should track abort handles in this test")
            .lock()
            .expect("tracker task lock poisoned");
        assert!(guard.is_empty(), "tracker task map should be empty after completion");
    }
}
