use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, EventBusError, EventHandler, HandlerResult, SyncEventHandler};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Work {
    value: usize,
}

// ── Handlers ─────────────────────────────────────────────────────────

struct NoOpSync;

impl SyncEventHandler<Work> for NoOpSync {
    fn handle(&self, _event: &Work) -> HandlerResult {
        Ok(())
    }
}

struct SyncAccumulator {
    sum: Arc<AtomicUsize>,
}

impl SyncEventHandler<Work> for SyncAccumulator {
    fn handle(&self, event: &Work) -> HandlerResult {
        self.sum.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

struct SlowAsync {
    done: Arc<AtomicUsize>,
}

impl EventHandler<Work> for SlowAsync {
    async fn handle(&self, _event: &Work) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.done.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn shutdown_stops_new_operations() {
    let bus = EventBus::new(16);
    bus.shutdown().await.expect("shutdown");

    let reg_err = bus.subscribe(NoOpSync).await.expect_err("subscribe after shutdown");
    assert_eq!(reg_err, EventBusError::ActorStopped);

    let pub_err = bus.publish(Work { value: 1 }).await.expect_err("publish after shutdown");
    assert_eq!(pub_err, EventBusError::ActorStopped);
}

#[tokio::test]
async fn unsubscribe_after_shutdown_returns_actor_stopped() {
    let bus = EventBus::new(16);
    let sum = Arc::new(AtomicUsize::new(0));

    let sub = bus.subscribe(SyncAccumulator { sum: Arc::clone(&sum) }).await.expect("subscribe");
    let id = sub.id();

    bus.shutdown().await.expect("shutdown");

    let err = bus.unsubscribe(id).await.expect_err("unsubscribe after shutdown");
    assert_eq!(err, EventBusError::ActorStopped);
}

#[tokio::test]
async fn shutdown_drains_queued_publishes() {
    let bus = EventBus::new(64);
    let sum = Arc::new(AtomicUsize::new(0));

    bus.subscribe(SyncAccumulator { sum: Arc::clone(&sum) }).await.expect("subscribe");

    // Enqueue several events via try_publish (non-blocking).
    for i in 1..=5 {
        bus.try_publish(Work { value: i }).expect("try_publish");
    }

    // Shutdown should drain the queued publishes.
    bus.shutdown().await.expect("shutdown");

    // 1+2+3+4+5 = 15
    assert_eq!(sum.load(Ordering::SeqCst), 15);
}

#[tokio::test]
async fn shutdown_waits_for_inflight_async_handlers() {
    let bus = EventBus::new(16);
    let done = Arc::new(AtomicUsize::new(0));

    bus.subscribe(SlowAsync { done: Arc::clone(&done) }).await.expect("subscribe");

    bus.publish(Work { value: 1 }).await.expect("publish");

    // Immediately shut down — the async handler is still sleeping.
    bus.shutdown().await.expect("shutdown");

    // Shutdown should have waited for the in-flight handler.
    assert_eq!(done.load(Ordering::SeqCst), 1);
}
