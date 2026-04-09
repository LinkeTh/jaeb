// SPDX-License-Identifier: MIT
//! Tests for the middleware / interceptor pipeline.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, EventBusError, HandlerResult, Middleware, MiddlewareDecision, SyncEventHandler, SyncMiddleware};

#[derive(Clone)]
struct Ping;

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn middleware_continues_handlers_fire() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _mw = bus.add_sync_middleware(AllowAll).await.expect("add middleware");
    let _sub = bus.subscribe::<Ping, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe");

    bus.publish(Ping).await.expect("publish should succeed");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 1, "handler should have fired");
}

#[tokio::test]
async fn middleware_rejects_handlers_do_not_fire() {
    let bus = EventBus::new(64).expect("valid config");
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
    let bus = EventBus::new(64).expect("valid config");
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
    let bus = EventBus::new(64).expect("valid config");
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

    let bus = EventBus::new(64).expect("valid config");
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

    let bus = EventBus::new(64).expect("valid config");
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

    let bus = EventBus::new(64).expect("valid config");
    let _mw = bus.add_middleware(AsyncReject).await.expect("add async middleware");

    let err = bus.publish(Ping).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(ref r) if r == "async rejection"));

    bus.shutdown().await.expect("shutdown");
}
