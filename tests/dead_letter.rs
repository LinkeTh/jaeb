use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use jaeb::{DeadLetter, EventBus, EventHandler, HandlerResult, NoRetryPolicy, SyncEventHandler};
use std::sync::Mutex;
use tokio::sync::Notify;

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
struct Alert(String);

// ── Handlers ─────────────────────────────────────────────────────────

struct AlwaysFailSync;

impl SyncEventHandler<Alert> for AlwaysFailSync {
    fn handle(&self, _event: &Alert) -> HandlerResult {
        Err("sync handler always fails".into())
    }
}

struct AlwaysFailAsync;

impl EventHandler<Alert> for AlwaysFailAsync {
    async fn handle(&self, _event: &Alert) -> HandlerResult {
        Err("async handler always fails".into())
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

struct DeadLetterCounter {
    seen: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl SyncEventHandler<DeadLetter> for DeadLetterCounter {
    fn handle(&self, _event: &DeadLetter) -> HandlerResult {
        self.seen.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_one();
        Ok(())
    }
}

struct FailingDeadLetterHandler;

impl SyncEventHandler<DeadLetter> for FailingDeadLetterHandler {
    fn handle(&self, _event: &DeadLetter) -> HandlerResult {
        Err("dead letter handler fails too".into())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn failed_handler_emits_dead_letter() {
    let bus = EventBus::new(16).expect("valid config");
    let notify = Arc::new(Notify::new());
    let seen = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe_dead_letters(DeadLetterCounter {
            seen: Arc::clone(&seen),
            notify: Arc::clone(&notify),
        })
        .await
        .expect("subscribe dead letters");

    let _ = bus.subscribe(AlwaysFailSync).await.expect("subscribe");

    bus.publish(Alert("boom".into())).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("timed out waiting for dead letter");

    assert_eq!(seen.load(Ordering::SeqCst), 1);
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn dead_letter_contains_correct_metadata() {
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

    let sub = bus.subscribe(AlwaysFailSync).await.expect("subscribe");
    let expected_id = sub.id();

    bus.publish(Alert("test".into())).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(1), notify.notified()).await.expect("timed out");

    let guard = letters.lock().unwrap();
    assert_eq!(guard.len(), 1);

    let dl = &guard[0];
    assert!(
        dl.event_name.contains("Alert"),
        "event_name should contain 'Alert', got: {}",
        dl.event_name
    );
    assert_eq!(dl.subscription_id, expected_id);
    assert_eq!(dl.attempts, 1); // no retries → 1 attempt
    assert!(dl.error.contains("sync handler always fails"), "error: {}", dl.error);
}

#[tokio::test]
async fn async_handler_failure_emits_dead_letter() {
    let bus = EventBus::new(16).expect("valid config");
    let seen = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe_dead_letters(DeadLetterCounter {
            seen: Arc::clone(&seen),
            notify: Arc::new(Notify::new()),
        })
        .await
        .expect("subscribe dead letters");

    let _ = bus.subscribe(AlwaysFailAsync).await.expect("subscribe");

    bus.publish(Alert("async boom".into())).await.expect("publish");

    // Shutdown drains in-flight async tasks and delivers their dead
    // letters directly to registered listeners (bypassing the channel).
    bus.shutdown().await.expect("shutdown");

    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dead_letter_suppressed_when_disabled() {
    let bus = EventBus::new(16).expect("valid config");
    let notify = Arc::new(Notify::new());
    let seen = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe_dead_letters(DeadLetterCounter {
            seen: Arc::clone(&seen),
            notify: Arc::clone(&notify),
        })
        .await
        .expect("subscribe dead letters");

    // Subscribe a handler with dead_letter = false.
    let policy = NoRetryPolicy::default().with_dead_letter(false);
    let _ = bus.subscribe_with_policy(AlwaysFailSync, policy).await.expect("subscribe");

    bus.publish(Alert("suppressed".into())).await.expect("publish");

    // Give the system time to process — no dead letter should arrive.
    let result = tokio::time::timeout(Duration::from_millis(100), notify.notified()).await;
    assert!(result.is_err(), "expected timeout (no dead letter), but notification arrived");

    assert_eq!(seen.load(Ordering::SeqCst), 0);
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn dead_letter_handler_failure_does_not_recurse() {
    let bus = EventBus::new(16).expect("valid config");

    // Subscribe a dead-letter handler that itself fails.
    let _ = bus
        .subscribe_dead_letters(FailingDeadLetterHandler)
        .await
        .expect("subscribe dead letters");

    // Register a normal handler that always fails.
    let _ = bus.subscribe(AlwaysFailSync).await.expect("subscribe");

    bus.publish(Alert("recurse?".into())).await.expect("publish");

    // If infinite recursion happened, the bus would hang or run out of resources.
    // We just need to successfully shut down within a reasonable timeout.
    tokio::time::timeout(Duration::from_secs(2), bus.shutdown())
        .await
        .expect("shutdown timed out — possible infinite recursion")
        .expect("shutdown");
}

#[tokio::test]
async fn dead_letter_contains_original_event() {
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

    let _ = bus.subscribe(AlwaysFailSync).await.expect("subscribe");

    bus.publish(Alert("payload-check".into())).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("timed out waiting for dead letter");

    let guard = letters.lock().unwrap();
    assert_eq!(guard.len(), 1);

    let dl = &guard[0];
    let original = dl.event.downcast_ref::<Alert>().expect("should downcast to Alert");
    assert_eq!(original.0, "payload-check");
}

#[tokio::test]
async fn dead_letter_has_timestamp() {
    let before = SystemTime::now();

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

    let _ = bus.subscribe(AlwaysFailSync).await.expect("subscribe");

    bus.publish(Alert("timestamp-check".into())).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("timed out waiting for dead letter");

    let after = SystemTime::now();

    let guard = letters.lock().unwrap();
    assert_eq!(guard.len(), 1);

    let dl = &guard[0];
    assert!(
        dl.failed_at >= before && dl.failed_at <= after,
        "failed_at ({:?}) should be between before ({:?}) and after ({:?})",
        dl.failed_at,
        before,
        after
    );
}

#[tokio::test]
async fn async_dead_letter_contains_original_event() {
    let bus = EventBus::new(16).expect("valid config");
    let letters: Arc<Mutex<Vec<DeadLetter>>> = Arc::default();

    let _ = bus
        .subscribe_dead_letters(DeadLetterCollector {
            letters: Arc::clone(&letters),
            notify: Arc::new(Notify::new()),
        })
        .await
        .expect("subscribe dead letters");

    let _ = bus.subscribe(AlwaysFailAsync).await.expect("subscribe");

    bus.publish(Alert("async-payload".into())).await.expect("publish");

    // Shutdown drains in-flight async tasks and delivers dead letters directly.
    bus.shutdown().await.expect("shutdown");

    let guard = letters.lock().unwrap();
    assert_eq!(guard.len(), 1);

    let dl = &guard[0];
    let original = dl.event.downcast_ref::<Alert>().expect("should downcast to Alert");
    assert_eq!(original.0, "async-payload");
    assert!(dl.failed_at <= SystemTime::now(), "failed_at should be in the past");
}
