use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Boom;

#[derive(Clone, Debug)]
struct Safe;

// ── Handlers ─────────────────────────────────────────────────────────

struct PanicHandler;

impl SyncEventHandler<Boom> for PanicHandler {
    fn handle(&self, _event: &Boom) -> HandlerResult {
        panic!("handler panicked on purpose");
    }
}

struct AsyncPanicHandler;

impl EventHandler<Boom> for AsyncPanicHandler {
    async fn handle(&self, _event: &Boom) -> HandlerResult {
        panic!("async handler panicked on purpose");
    }
}

struct SafeCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<Safe> for SafeCounter {
    fn handle(&self, event: &Safe) -> HandlerResult {
        let _ = event;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

/// A panicking sync handler must not crash the bus.  Subsequent publishes to
/// a different handler should still work.
///
/// NOTE: this test will print a panic backtrace to stderr — that is expected.
#[tokio::test]
async fn panicking_handler_does_not_crash_bus() {
    let bus = EventBus::new(16).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(PanicHandler).await.expect("subscribe panic handler");
    let _ = bus
        .subscribe(SafeCounter { count: Arc::clone(&count) })
        .await
        .expect("subscribe safe handler");

    // Publish the event that triggers a panic.
    bus.publish(Boom).await.expect("publish Boom");

    // The bus should still be alive. Publish to a different event type.
    bus.publish(Safe).await.expect("publish Safe after panic");

    assert_eq!(count.load(Ordering::SeqCst), 1);
    bus.shutdown().await.expect("shutdown");
}

/// An async handler that panics should not crash the bus.  The panic is caught
/// by the JoinSet and fed through the dead-letter pipeline.
///
/// NOTE: this test will print a panic backtrace to stderr — that is expected.
#[tokio::test]
async fn async_panicking_handler_does_not_crash_bus() {
    let bus = EventBus::new(16).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(AsyncPanicHandler).await.expect("subscribe async panic handler");
    let _ = bus
        .subscribe(SafeCounter { count: Arc::clone(&count) })
        .await
        .expect("subscribe safe handler");

    // Publish the event that triggers a panic in the async handler.
    bus.publish(Boom).await.expect("publish Boom");

    // The bus should still be alive. Publish to a different event type.
    bus.publish(Safe).await.expect("publish Safe after async panic");

    assert_eq!(count.load(Ordering::SeqCst), 1);

    // Shutdown to ensure the async panic task is reaped cleanly.
    bus.shutdown().await.expect("shutdown");
}
