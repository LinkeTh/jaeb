use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use jaeb::{DeadLetter, EventBus, EventHandler, HandlerResult, SyncEventHandler};
use tokio::sync::Notify;

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Task(#[allow(dead_code)] String);

// ── Handlers ─────────────────────────────────────────────────────────

struct NamedSyncHandler;

impl SyncEventHandler<Task> for NamedSyncHandler {
    fn handle(&self, _event: &Task) -> HandlerResult {
        Err("named sync handler fails".into())
    }

    fn name(&self) -> Option<&'static str> {
        Some("audit-logger")
    }
}

struct NamedAsyncHandler;

impl EventHandler<Task> for NamedAsyncHandler {
    async fn handle(&self, _event: &Task) -> HandlerResult {
        Err("named async handler fails".into())
    }

    fn name(&self) -> Option<&'static str> {
        Some("async-worker")
    }
}

struct UnnamedSyncHandler;

impl SyncEventHandler<Task> for UnnamedSyncHandler {
    fn handle(&self, _event: &Task) -> HandlerResult {
        Err("unnamed handler fails".into())
    }
}

struct DeadLetterCollector {
    letters: Arc<Mutex<Vec<DeadLetter>>>,
    notify: Arc<Notify>,
}

impl SyncEventHandler<DeadLetter> for DeadLetterCollector {
    fn handle(&self, event: &DeadLetter) -> HandlerResult {
        let mut guard = self.letters.lock().unwrap();
        guard.push(event.clone());
        self.notify.notify_one();
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn named_sync_handler_appears_in_dead_letter() {
    let bus = EventBus::new(16).expect("valid config");
    let notify = Arc::new(Notify::new());
    let letters: Arc<Mutex<Vec<DeadLetter>>> = Arc::default();

    let _ = bus
        .subscribe_dead_letters(DeadLetterCollector {
            letters: Arc::clone(&letters),
            notify: Arc::clone(&notify),
        })
        .await
        .expect("subscribe dead letters");

    let _ = bus.subscribe(NamedSyncHandler).await.expect("subscribe");

    bus.publish(Task("test".into())).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("timed out waiting for dead letter");

    let guard = letters.lock().unwrap();
    assert_eq!(guard.len(), 1);
    assert_eq!(guard[0].listener_name, Some("audit-logger"));
}

#[tokio::test]
async fn named_async_handler_appears_in_dead_letter() {
    let bus = EventBus::new(16).expect("valid config");
    let letters: Arc<Mutex<Vec<DeadLetter>>> = Arc::default();

    let _ = bus
        .subscribe_dead_letters(DeadLetterCollector {
            letters: Arc::clone(&letters),
            notify: Arc::new(Notify::new()),
        })
        .await
        .expect("subscribe dead letters");

    let _ = bus.subscribe(NamedAsyncHandler).await.expect("subscribe");

    bus.publish(Task("test".into())).await.expect("publish");

    // Shutdown drains in-flight async tasks and delivers dead letters directly.
    bus.shutdown().await.expect("shutdown");

    let guard = letters.lock().unwrap();
    assert_eq!(guard.len(), 1);
    assert_eq!(guard[0].listener_name, Some("async-worker"));
}

#[tokio::test]
async fn unnamed_handler_defaults_to_none() {
    let bus = EventBus::new(16).expect("valid config");
    let notify = Arc::new(Notify::new());
    let letters: Arc<Mutex<Vec<DeadLetter>>> = Arc::default();

    let _ = bus
        .subscribe_dead_letters(DeadLetterCollector {
            letters: Arc::clone(&letters),
            notify: Arc::clone(&notify),
        })
        .await
        .expect("subscribe dead letters");

    let _ = bus.subscribe(UnnamedSyncHandler).await.expect("subscribe");

    bus.publish(Task("test".into())).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("timed out waiting for dead letter");

    let guard = letters.lock().unwrap();
    assert_eq!(guard.len(), 1);
    assert_eq!(guard[0].listener_name, None);
}
