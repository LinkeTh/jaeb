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

struct VerySlowAsync {
    done: Arc<AtomicUsize>,
}

impl EventHandler<Work> for VerySlowAsync {
    async fn handle(&self, _event: &Work) -> HandlerResult {
        tokio::time::sleep(Duration::from_secs(5)).await;
        self.done.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn shutdown_stops_new_operations() {
    let bus = EventBus::new(16).expect("valid config");
    bus.shutdown().await.expect("shutdown");

    let reg_err = match bus.subscribe(NoOpSync).await {
        Ok(_) => panic!("subscribe after shutdown unexpectedly succeeded"),
        Err(err) => err,
    };
    assert_eq!(reg_err, EventBusError::Stopped);

    let pub_err = bus.publish(Work { value: 1 }).await.expect_err("publish after shutdown");
    assert_eq!(pub_err, EventBusError::Stopped);
}

#[tokio::test]
async fn unsubscribe_after_shutdown_returns_stopped() {
    let bus = EventBus::new(16).expect("valid config");
    let sum = Arc::new(AtomicUsize::new(0));

    let sub = bus.subscribe(SyncAccumulator { sum: Arc::clone(&sum) }).await.expect("subscribe");
    let id = sub.id();

    bus.shutdown().await.expect("shutdown");

    let err = bus.unsubscribe(id).await.expect_err("unsubscribe after shutdown");
    assert_eq!(err, EventBusError::Stopped);
}

#[tokio::test]
async fn shutdown_drains_queued_publishes() {
    let bus = EventBus::new(64).expect("valid config");
    let sum = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SyncAccumulator { sum: Arc::clone(&sum) }).await.expect("subscribe");

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
    let bus = EventBus::new(16).expect("valid config");
    let done = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowAsync { done: Arc::clone(&done) }).await.expect("subscribe");

    bus.publish(Work { value: 1 }).await.expect("publish");

    // Immediately shut down — the async handler is still sleeping.
    bus.shutdown().await.expect("shutdown");

    // Shutdown should have waited for the in-flight handler.
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shutdown_returns_timeout_when_tasks_aborted() {
    let bus = EventBus::builder()
        .buffer_size(16)
        .shutdown_timeout(Duration::from_millis(50))
        .build()
        .expect("valid config");

    let done = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(VerySlowAsync { done: Arc::clone(&done) }).await.expect("subscribe");

    bus.publish(Work { value: 1 }).await.expect("publish");

    // The async handler sleeps for 5s but timeout is 50ms — tasks will be aborted.
    let result = bus.shutdown().await;
    assert_eq!(result, Err(EventBusError::ShutdownTimeout));

    // The handler should not have completed.
    assert_eq!(done.load(Ordering::SeqCst), 0);

    // The bus should be fully stopped afterward.
    let err = bus.publish(Work { value: 2 }).await.expect_err("publish after shutdown");
    assert_eq!(err, EventBusError::Stopped);
}

#[tokio::test]
async fn shutdown_succeeds_when_tasks_finish_before_deadline() {
    let bus = EventBus::builder()
        .buffer_size(16)
        .shutdown_timeout(Duration::from_secs(5))
        .build()
        .expect("valid config");

    let done = Arc::new(AtomicUsize::new(0));

    // SlowAsync sleeps 100ms, well within the 5s deadline.
    let _ = bus.subscribe(SlowAsync { done: Arc::clone(&done) }).await.expect("subscribe");

    bus.publish(Work { value: 1 }).await.expect("publish");

    bus.shutdown().await.expect("shutdown should succeed when tasks finish in time");

    assert_eq!(done.load(Ordering::SeqCst), 1);
}
