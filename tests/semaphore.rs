use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, EventHandler, HandlerResult};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Ping;

// ── Handlers ─────────────────────────────────────────────────────────

struct SlowHandler {
    count: Arc<AtomicUsize>,
}

impl EventHandler<Ping> for SlowHandler {
    async fn handle(&self, _event: &Ping) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

/// When async concurrency is limited and shutdown aborts waiting tasks,
/// the semaphore path must not panic — it should gracefully skip
/// execution for tasks that cannot acquire a permit.
#[tokio::test]
async fn semaphore_limited_bus_shuts_down_cleanly() {
    let bus = EventBus::builder()
        .buffer_size(64)
        .max_concurrent_async(1)
        .shutdown_timeout(Duration::from_millis(100))
        .build()
        .expect("valid config");

    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowHandler { count: Arc::clone(&count) }).await.expect("subscribe");

    // Publish several events — with concurrency limit of 1, most will be
    // queued behind the semaphore.
    for _ in 0..5 {
        bus.publish(Ping).await.expect("publish");
    }

    // Shutdown with a short timeout — this will abort tasks still waiting
    // for a semaphore permit. The important assertion is that no panic occurs.
    let _ = bus.shutdown().await;

    // At least the first handler should have run.
    assert!(count.load(Ordering::SeqCst) >= 1);
}

/// Verify that async handlers behind a semaphore execute correctly
/// under normal (non-shutdown) conditions.
#[tokio::test]
async fn semaphore_limited_handlers_execute_normally() {
    let bus = EventBus::builder().buffer_size(64).max_concurrent_async(2).build().expect("valid config");

    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowHandler { count: Arc::clone(&count) }).await.expect("subscribe");

    for _ in 0..4 {
        bus.publish(Ping).await.expect("publish");
    }

    // Shutdown drains in-flight async handlers deterministically.
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 4);
}
