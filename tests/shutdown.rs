use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, EventBusError, EventHandler, HandlerResult, SyncEventHandler, SyncMiddleware};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Work {
    value: usize,
}

// ── Handlers ─────────────────────────────────────────────────────────

struct NoOpSync;

impl SyncEventHandler<Work> for NoOpSync {
    fn handle(&self, _event: &Work, _bus: &EventBus) -> HandlerResult {
        Ok(())
    }
}

struct SyncAccumulator {
    sum: Arc<AtomicUsize>,
}

impl SyncEventHandler<Work> for SyncAccumulator {
    fn handle(&self, event: &Work, _bus: &EventBus) -> HandlerResult {
        self.sum.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

struct SlowAsync {
    done: Arc<AtomicUsize>,
}

impl EventHandler<Work> for SlowAsync {
    async fn handle(&self, _event: &Work, _bus: &EventBus) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.done.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct VerySlowAsync {
    done: Arc<AtomicUsize>,
}

impl EventHandler<Work> for VerySlowAsync {
    async fn handle(&self, _event: &Work, _bus: &EventBus) -> HandlerResult {
        tokio::time::sleep(Duration::from_secs(5)).await;
        self.done.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct BlockingAsyncMiddleware {
    started: Arc<AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

impl jaeb::Middleware for BlockingAsyncMiddleware {
    async fn process(&self, _name: &'static str, _event: &(dyn std::any::Any + Send + Sync)) -> jaeb::MiddlewareDecision {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        jaeb::MiddlewareDecision::Continue
    }
}

struct SleepingSyncMiddleware {
    started: Arc<AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

impl SyncMiddleware for SleepingSyncMiddleware {
    fn process(&self, _name: &'static str, _event: &(dyn std::any::Any + Send + Sync)) -> jaeb::MiddlewareDecision {
        self.started.fetch_add(1, Ordering::SeqCst);
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.release.notified()));
        jaeb::MiddlewareDecision::Continue
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn shutdown_stops_new_operations() {
    let bus = EventBus::builder().build().await.expect("valid config");
    tokio::time::timeout(Duration::from_secs(2), bus.shutdown())
        .await
        .expect("shutdown timed out")
        .expect("shutdown");

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
    let bus = EventBus::builder().build().await.expect("valid config");
    let sum = Arc::new(AtomicUsize::new(0));

    let sub = bus.subscribe(SyncAccumulator { sum: Arc::clone(&sum) }).await.expect("subscribe");
    let id = sub.id();

    tokio::time::timeout(Duration::from_secs(2), bus.shutdown())
        .await
        .expect("shutdown timed out")
        .expect("shutdown");

    let err = bus.unsubscribe(id).await.expect_err("unsubscribe after shutdown");
    assert_eq!(err, EventBusError::Stopped);
}

#[tokio::test]
async fn shutdown_waits_for_inflight_async_handlers() {
    let bus = EventBus::builder().build().await.expect("valid config");
    let done = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowAsync { done: Arc::clone(&done) }).await.expect("subscribe");

    bus.publish(Work { value: 1 }).await.expect("publish");

    // Immediately shut down — the async handler is still sleeping.
    tokio::time::timeout(Duration::from_secs(3), bus.shutdown())
        .await
        .expect("shutdown timed out")
        .expect("shutdown");

    // Shutdown should have waited for the in-flight handler.
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shutdown_returns_timeout_when_tasks_aborted() {
    let bus = EventBus::builder()
        .shutdown_timeout(Duration::from_millis(50))
        .build()
        .await
        .expect("valid config");

    let done = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(VerySlowAsync { done: Arc::clone(&done) }).await.expect("subscribe");

    bus.publish(Work { value: 1 }).await.expect("publish");

    // The async handler sleeps for 5s but timeout is 50ms — tasks will be aborted.
    let result = tokio::time::timeout(Duration::from_secs(2), bus.shutdown())
        .await
        .expect("shutdown call timed out");
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
        .shutdown_timeout(Duration::from_secs(5))
        .build()
        .await
        .expect("valid config");

    let done = Arc::new(AtomicUsize::new(0));

    // SlowAsync sleeps 100ms, well within the 5s deadline.
    let _ = bus.subscribe(SlowAsync { done: Arc::clone(&done) }).await.expect("subscribe");

    bus.publish(Work { value: 1 }).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(3), bus.shutdown())
        .await
        .expect("shutdown timed out")
        .expect("shutdown should succeed when tasks finish in time");

    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shutdown_timeout_blocks_async_spawns_from_accepted_publish_still_in_middleware() {
    let bus = EventBus::builder()
        .shutdown_timeout(Duration::from_millis(50))
        .build()
        .await
        .expect("valid config");

    let middleware_started = Arc::new(AtomicUsize::new(0));
    let middleware_release = Arc::new(tokio::sync::Notify::new());
    let handler_done = Arc::new(AtomicUsize::new(0));

    let _mw = bus
        .add_middleware(BlockingAsyncMiddleware {
            started: Arc::clone(&middleware_started),
            release: Arc::clone(&middleware_release),
        })
        .await
        .expect("add middleware");

    let _sub = bus
        .subscribe(VerySlowAsync {
            done: Arc::clone(&handler_done),
        })
        .await
        .expect("subscribe");

    let publish_task = tokio::spawn({
        let bus = bus.clone();
        async move { bus.publish(Work { value: 1 }).await }
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        while middleware_started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("middleware should start");

    let result = tokio::time::timeout(Duration::from_secs(2), bus.shutdown())
        .await
        .expect("shutdown call timed out");
    assert_eq!(result, Err(EventBusError::ShutdownTimeout));

    middleware_release.notify_waiters();

    let publish_result = tokio::time::timeout(Duration::from_secs(2), publish_task)
        .await
        .expect("publish task timed out")
        .expect("publish task panicked");
    assert_eq!(publish_result, Ok(()));

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        handler_done.load(Ordering::SeqCst),
        0,
        "async handler should not spawn after shutdown timeout"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_timeout_rejects_async_spawn_after_sync_middleware_completes() {
    let bus = EventBus::builder()
        .shutdown_timeout(Duration::from_millis(50))
        .build()
        .await
        .expect("valid config");

    let middleware_started = Arc::new(AtomicUsize::new(0));
    let middleware_release = Arc::new(tokio::sync::Notify::new());
    let handler_done = Arc::new(AtomicUsize::new(0));

    let _mw = bus
        .add_sync_middleware(SleepingSyncMiddleware {
            started: Arc::clone(&middleware_started),
            release: Arc::clone(&middleware_release),
        })
        .await
        .expect("add middleware");

    let _sub = bus
        .subscribe(VerySlowAsync {
            done: Arc::clone(&handler_done),
        })
        .await
        .expect("subscribe");

    let publish_task = tokio::spawn({
        let bus = bus.clone();
        async move { bus.publish(Work { value: 1 }).await }
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        while middleware_started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("middleware should start");

    let result = tokio::time::timeout(Duration::from_secs(2), bus.shutdown())
        .await
        .expect("shutdown call timed out");
    assert_eq!(result, Err(EventBusError::ShutdownTimeout));

    middleware_release.notify_waiters();

    let publish_result = tokio::time::timeout(Duration::from_secs(2), publish_task)
        .await
        .expect("publish task timed out")
        .expect("publish task panicked");
    assert_eq!(publish_result, Ok(()));

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        handler_done.load(Ordering::SeqCst),
        0,
        "async handler should not spawn after timeout rejection"
    );
}
