//! Tests for the middleware / interceptor pipeline.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{
    EventBus, EventBusError, HandlerResult, Middleware, MiddlewareDecision, SyncEventHandler, SyncMiddleware, TypedMiddleware, TypedSyncMiddleware,
};

#[derive(Clone)]
struct Ping;

#[derive(Clone)]
struct Pong;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct AllowAll;
impl SyncMiddleware for AllowAll {
    fn process(&self, _name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        MiddlewareDecision::Continue
    }
}

struct RejectAll(String);
impl SyncMiddleware for RejectAll {
    fn process(&self, _name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        MiddlewareDecision::Reject(self.0.clone())
    }
}

struct Counter(Arc<AtomicUsize>);

impl SyncEventHandler<Ping> for Counter {
    fn handle(&self, _event: &Ping) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct PongCounter(Arc<AtomicUsize>);

impl SyncEventHandler<Pong> for PongCounter {
    fn handle(&self, _event: &Pong) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct OrderTracker {
    id: &'static str,
    log: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl SyncMiddleware for OrderTracker {
    fn process(&self, _name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        self.log.lock().unwrap().push(self.id);
        MiddlewareDecision::Continue
    }
}

struct AsyncOrderTracker {
    id: &'static str,
    log: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl Middleware for AsyncOrderTracker {
    async fn process(&self, _name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        self.log.lock().unwrap().push(self.id);
        MiddlewareDecision::Continue
    }
}

struct TypedPingCounter(Arc<AtomicUsize>);

impl TypedSyncMiddleware<Ping> for TypedPingCounter {
    fn process(&self, _event_name: &'static str, _event: &Ping) -> MiddlewareDecision {
        self.0.fetch_add(1, Ordering::SeqCst);
        MiddlewareDecision::Continue
    }
}

struct TypedSyncReject(&'static str);

impl TypedSyncMiddleware<Ping> for TypedSyncReject {
    fn process(&self, _event_name: &'static str, _event: &Ping) -> MiddlewareDecision {
        MiddlewareDecision::Reject(self.0.to_string())
    }
}

struct TypedAsyncReject(&'static str);

impl TypedMiddleware<Ping> for TypedAsyncReject {
    async fn process(&self, _event_name: &'static str, _event: &Ping) -> MiddlewareDecision {
        MiddlewareDecision::Reject(self.0.to_string())
    }
}

struct TypedSyncOrderTracker {
    id: &'static str,
    log: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl TypedSyncMiddleware<Ping> for TypedSyncOrderTracker {
    fn process(&self, _event_name: &'static str, _event: &Ping) -> MiddlewareDecision {
        self.log.lock().unwrap().push(self.id);
        MiddlewareDecision::Continue
    }
}

struct TypedAsyncOrderTracker {
    id: &'static str,
    log: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

struct AlwaysFailPing;

impl SyncEventHandler<Ping> for AlwaysFailPing {
    fn handle(&self, _event: &Ping) -> HandlerResult {
        Err("sync failure".into())
    }
}

struct DeadLetterCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<jaeb::DeadLetter> for DeadLetterCounter {
    fn handle(&self, _event: &jaeb::DeadLetter) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl TypedMiddleware<Ping> for TypedAsyncOrderTracker {
    async fn process(&self, _event_name: &'static str, _event: &Ping) -> MiddlewareDecision {
        self.log.lock().unwrap().push(self.id);
        MiddlewareDecision::Continue
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn middleware_continues_handlers_fire() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _mw = bus.add_sync_middleware(AllowAll).await.expect("add middleware");
    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    bus.publish(Ping).await.expect("publish should succeed");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 1, "handler should have fired");
}

#[tokio::test]
async fn middleware_rejects_handlers_do_not_fire() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _mw = bus.add_sync_middleware(RejectAll("blocked".into())).await.expect("add middleware");
    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    let err = bus.publish(Ping).await.unwrap_err();
    assert!(
        matches!(err, EventBusError::MiddlewareRejected(ref reason) if reason == "blocked"),
        "expected MiddlewareRejected, got: {err:?}"
    );

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 0, "handler should NOT have fired");
}

#[tokio::test]
async fn middleware_ordering_fifo() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let log = Arc::new(std::sync::Mutex::new(Vec::<&str>::new()));
    let count = Arc::new(AtomicUsize::new(0));

    let _mw1 = bus
        .add_sync_middleware(OrderTracker {
            id: "first",
            log: Arc::clone(&log),
        })
        .await
        .expect("add middleware 1");

    let _mw2 = bus
        .add_sync_middleware(OrderTracker {
            id: "second",
            log: Arc::clone(&log),
        })
        .await
        .expect("add middleware 2");

    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    bus.publish(Ping).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let entries = log.lock().unwrap().clone();
    assert_eq!(entries, vec!["first", "second"], "middlewares should execute in FIFO order");
    assert_eq!(count.load(Ordering::SeqCst), 1, "handler should have fired");
}

#[tokio::test]
async fn middleware_removal() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let mw_sub = bus.add_sync_middleware(RejectAll("blocked".into())).await.expect("add middleware");
    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    // Should be rejected.
    let err = bus.publish(Ping).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(_)));

    // Remove the middleware.
    let removed = mw_sub.unsubscribe().await.expect("unsubscribe middleware");
    assert!(removed, "middleware should have been found and removed");

    // Now publish should succeed.
    bus.publish(Ping).await.expect("publish after removal");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 1, "handler should fire after middleware removed");
}

#[tokio::test]
async fn middleware_with_downcast() {
    #[derive(Clone)]
    struct ImportantEvent(u32);

    #[derive(Clone)]
    struct IgnoredEvent;

    struct OnlyAllowImportant;
    impl SyncMiddleware for OnlyAllowImportant {
        fn process(&self, _name: &'static str, event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
            if let Some(e) = event.downcast_ref::<ImportantEvent>()
                && e.0 > 10
            {
                return MiddlewareDecision::Reject("value too high".into());
            }
            MiddlewareDecision::Continue
        }
    }

    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let _mw = bus.add_sync_middleware(OnlyAllowImportant).await.expect("add middleware");

    // ImportantEvent(5) should pass.
    bus.publish(ImportantEvent(5)).await.expect("low value should pass");

    // ImportantEvent(20) should be rejected.
    let err = bus.publish(ImportantEvent(20)).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(ref r) if r == "value too high"));

    // IgnoredEvent should pass (not an ImportantEvent, middleware says Continue).
    bus.publish(IgnoredEvent).await.expect("ignored event should pass");

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn async_middleware_works() {
    struct AsyncAllow;
    impl Middleware for AsyncAllow {
        async fn process(&self, _name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
            // Simulate async work
            tokio::task::yield_now().await;
            MiddlewareDecision::Continue
        }
    }

    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _mw = bus.add_middleware(AsyncAllow).await.expect("add async middleware");
    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    bus.publish(Ping).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 1, "handler should have fired through async middleware");
}

#[tokio::test]
async fn async_middleware_rejects() {
    struct AsyncReject;
    impl Middleware for AsyncReject {
        async fn process(&self, _name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
            MiddlewareDecision::Reject("async rejection".into())
        }
    }

    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let _mw = bus.add_middleware(AsyncReject).await.expect("add async middleware");

    let err = bus.publish(Ping).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(ref r) if r == "async rejection"));

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn typed_middleware_runs_for_matching_event_only() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let typed_hits = Arc::new(AtomicUsize::new(0));
    let ping_count = Arc::new(AtomicUsize::new(0));
    let pong_count = Arc::new(AtomicUsize::new(0));

    let _typed = bus
        .add_typed_sync_middleware::<Ping, _>(TypedPingCounter(Arc::clone(&typed_hits)))
        .await
        .expect("add typed middleware");

    let _ping = bus
        .subscribe::<Ping, _, _>(Counter(Arc::clone(&ping_count)))
        .await
        .expect("subscribe ping");
    let _pong = bus
        .subscribe::<Pong, _, _>(PongCounter(Arc::clone(&pong_count)))
        .await
        .expect("subscribe pong");

    bus.publish(Ping).await.expect("publish ping");
    bus.publish(Pong).await.expect("publish pong");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(typed_hits.load(Ordering::SeqCst), 1, "typed middleware should only run for Ping");
    assert_eq!(ping_count.load(Ordering::SeqCst), 1, "Ping handler should fire");
    assert_eq!(pong_count.load(Ordering::SeqCst), 1, "Pong handler should fire");
}

#[tokio::test]
async fn typed_sync_middleware_rejects_before_handlers() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _typed = bus
        .add_typed_sync_middleware::<Ping, _>(TypedSyncReject("typed sync reject"))
        .await
        .expect("add typed middleware");

    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    let err = bus.publish(Ping).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(ref r) if r == "typed sync reject"));

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 0, "handler should not fire after typed sync rejection");
}

#[tokio::test]
async fn typed_async_middleware_rejects_before_handlers() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _typed = bus
        .add_typed_middleware::<Ping, _>(TypedAsyncReject("typed async reject"))
        .await
        .expect("add typed middleware");

    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    let err = bus.publish(Ping).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(ref r) if r == "typed async reject"));

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 0, "handler should not fire after typed async rejection");
}

#[tokio::test]
async fn typed_middleware_runs_after_global_middleware_in_order() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let log = Arc::new(std::sync::Mutex::new(Vec::<&str>::new()));

    let _global_sync = bus
        .add_sync_middleware(OrderTracker {
            id: "global-sync",
            log: Arc::clone(&log),
        })
        .await
        .expect("add global sync middleware");

    let _global_async = bus
        .add_middleware(AsyncOrderTracker {
            id: "global-async",
            log: Arc::clone(&log),
        })
        .await
        .expect("add global async middleware");

    let _typed_sync = bus
        .add_typed_sync_middleware::<Ping, _>(TypedSyncOrderTracker {
            id: "typed-sync",
            log: Arc::clone(&log),
        })
        .await
        .expect("add typed sync middleware");

    let _typed_async = bus
        .add_typed_middleware::<Ping, _>(TypedAsyncOrderTracker {
            id: "typed-async",
            log: Arc::clone(&log),
        })
        .await
        .expect("add typed async middleware");

    let handler_log = Arc::clone(&log);
    let _sub = bus
        .subscribe::<Ping, _, _>(move |_event: &Ping| {
            handler_log.lock().unwrap().push("handler");
            Ok(())
        })
        .await
        .expect("subscribe handler");

    bus.publish(Ping).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let entries = log.lock().unwrap().clone();
    assert_eq!(entries, vec!["global-sync", "global-async", "typed-sync", "typed-async", "handler"]);
}

#[tokio::test]
async fn typed_middleware_ordering_fifo() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let log = Arc::new(std::sync::Mutex::new(Vec::<&str>::new()));

    let _typed_a = bus
        .add_typed_sync_middleware::<Ping, _>(TypedSyncOrderTracker {
            id: "typed-a",
            log: Arc::clone(&log),
        })
        .await
        .expect("add typed middleware a");

    let _typed_b = bus
        .add_typed_sync_middleware::<Ping, _>(TypedSyncOrderTracker {
            id: "typed-b",
            log: Arc::clone(&log),
        })
        .await
        .expect("add typed middleware b");

    let handler_log = Arc::clone(&log);
    let _sub = bus
        .subscribe::<Ping, _, _>(move |_event: &Ping| {
            handler_log.lock().unwrap().push("handler");
            Ok(())
        })
        .await
        .expect("subscribe handler");

    bus.publish(Ping).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let entries = log.lock().unwrap().clone();
    assert_eq!(entries, vec!["typed-a", "typed-b", "handler"], "typed middlewares should run FIFO");
}

#[tokio::test]
async fn typed_middleware_removal() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let typed_sub = bus
        .add_typed_sync_middleware::<Ping, _>(TypedSyncReject("typed blocked"))
        .await
        .expect("add typed middleware");

    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    let err = bus.publish(Ping).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(ref r) if r == "typed blocked"));

    let removed = typed_sub.unsubscribe().await.expect("unsubscribe typed middleware");
    assert!(removed, "typed middleware should be removed");

    bus.publish(Ping).await.expect("publish after typed middleware removal");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 1, "handler should fire once middleware is removed");
}

#[tokio::test]
async fn sync_only_fast_path_preserves_pipeline_and_dead_letter() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let log = Arc::new(std::sync::Mutex::new(Vec::<&str>::new()));
    let dead_letters = Arc::new(AtomicUsize::new(0));

    let _dl = bus
        .subscribe_dead_letters(DeadLetterCounter {
            count: Arc::clone(&dead_letters),
        })
        .await
        .expect("subscribe dead letters");

    let _global_sync = bus
        .add_sync_middleware(OrderTracker {
            id: "global-sync",
            log: Arc::clone(&log),
        })
        .await
        .expect("add global sync middleware");

    let _typed_sync = bus
        .add_typed_sync_middleware::<Ping, _>(TypedSyncOrderTracker {
            id: "typed-sync",
            log: Arc::clone(&log),
        })
        .await
        .expect("add typed sync middleware");

    let handler_log = Arc::clone(&log);
    let _sub = bus
        .subscribe::<Ping, _, _>(move |_event: &Ping| {
            handler_log.lock().unwrap().push("handler");
            Ok(())
        })
        .await
        .expect("subscribe sync handler");

    let _failing = bus.subscribe::<Ping, _, _>(AlwaysFailPing).await.expect("subscribe failing handler");

    bus.publish(Ping).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let entries = log.lock().unwrap().clone();
    assert_eq!(&entries[..3], ["global-sync", "typed-sync", "handler"]);
    assert_eq!(entries.iter().filter(|entry| **entry == "global-sync").count(), 2);
    assert_eq!(dead_letters.load(Ordering::SeqCst), 1);
}
