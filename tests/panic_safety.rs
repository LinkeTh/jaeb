use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, HandlerResult, SyncEventHandler};

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

/// A panicking handler must not crash the bus.  Subsequent publishes to
/// a different handler should still work.
///
/// NOTE: this test will print a panic backtrace to stderr — that is expected.
#[tokio::test]
async fn panicking_handler_does_not_crash_bus() {
    let bus = EventBus::new(16);
    let count = Arc::new(AtomicUsize::new(0));

    bus.register(PanicHandler).await.expect("register panic handler");
    bus.register(SafeCounter { count: Arc::clone(&count) })
        .await
        .expect("register safe handler");

    // Publish the event that triggers a panic.
    bus.publish(Boom).await.expect("publish Boom");

    // Give the runtime a moment to process the panic.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The bus should still be alive. Publish to a different event type.
    bus.publish(Safe).await.expect("publish Safe after panic");

    assert_eq!(count.load(Ordering::SeqCst), 1);
    bus.shutdown().await.expect("shutdown");
}
