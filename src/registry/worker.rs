use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Semaphore, mpsc};

use super::dispatch::{AsyncListenerMeta, execute_async_listener};
use super::tracker::AsyncTaskTracker;
use super::types::{ControlNotification, ErasedAsyncHandlerFn, EventType};

struct ExternalWorkGuard {
    tracker: Arc<AsyncTaskTracker>,
}

impl ExternalWorkGuard {
    fn new(tracker: Arc<AsyncTaskTracker>) -> Self {
        tracker.track_external();
        Self { tracker }
    }
}

impl Drop for ExternalWorkGuard {
    fn drop(&mut self) {
        self.tracker.finish_external();
    }
}

pub(crate) struct WorkItem {
    pub handler: ErasedAsyncHandlerFn,
    pub event: EventType,
    pub event_name: &'static str,
    pub meta: AsyncListenerMeta,
    pub handler_timeout: Option<Duration>,
    pub notify_tx: mpsc::UnboundedSender<ControlNotification>,
    pub concurrency_semaphore: Option<Arc<Semaphore>>,
    _guard: ExternalWorkGuard,
}

pub(crate) struct WorkItemData {
    pub handler: ErasedAsyncHandlerFn,
    pub event: EventType,
    pub event_name: &'static str,
    pub meta: AsyncListenerMeta,
    pub handler_timeout: Option<Duration>,
    pub notify_tx: mpsc::UnboundedSender<ControlNotification>,
    pub concurrency_semaphore: Option<Arc<Semaphore>>,
}

impl WorkItem {
    pub(crate) fn from_data(data: WorkItemData, tracker: Arc<AsyncTaskTracker>) -> Self {
        Self {
            handler: data.handler,
            event: data.event,
            event_name: data.event_name,
            meta: data.meta,
            handler_timeout: data.handler_timeout,
            notify_tx: data.notify_tx,
            concurrency_semaphore: data.concurrency_semaphore,
            _guard: ExternalWorkGuard::new(tracker),
        }
    }
}

/// A persistent worker task that processes async handler invocations without
/// per-publish `tokio::spawn()` overhead. One worker is created per event-type
/// slot when it has async listeners.
#[derive(Clone)]
pub(crate) struct AsyncSlotWorker {
    tx: mpsc::UnboundedSender<WorkItem>,
}

impl AsyncSlotWorker {
    /// Create a new worker and spawn its processing loop.
    pub(crate) fn spawn(_tracker: Arc<AsyncTaskTracker>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(worker_loop(rx));
        Self { tx }
    }

    /// Send a work item to the persistent worker. Returns `true` if sent
    /// successfully, `false` if the worker has been shut down.
    pub(crate) fn send(&self, item: WorkItem) -> bool {
        self.tx.send(item).is_ok()
    }
}

async fn worker_loop(mut rx: mpsc::UnboundedReceiver<WorkItem>) {
    while let Some(item) = rx.recv().await {
        let task = async {
            if let Some(failure) = execute_async_listener(item.handler, item.event, item.event_name, item.meta, item.handler_timeout).await {
                let _ = item.notify_tx.send(ControlNotification::Failure(failure));
            }
        };

        if let Some(semaphore) = item.concurrency_semaphore {
            if let Ok(_permit) = semaphore.acquire().await {
                task.await;
            }
        } else {
            task.await;
        }
    }
}
