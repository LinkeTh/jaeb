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

struct SuccessfulNamedHandler;

impl SyncEventHandler<Task> for SuccessfulNamedHandler {
    fn handle(&self, _event: &Task) -> HandlerResult {
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("success-handler")
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
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
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
    assert_eq!(guard[0].handler_name, Some("audit-logger"));
}

#[tokio::test]
async fn named_async_handler_appears_in_dead_letter() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
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
    assert_eq!(guard[0].handler_name, Some("async-worker"));
}

#[tokio::test]
async fn unnamed_handler_defaults_to_none() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
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
    assert_eq!(guard[0].handler_name, None);
}

// ── Subscription / SubscriptionGuard name query tests ───────────────

#[tokio::test]
async fn subscription_name_returns_handler_name() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let sub = bus.subscribe(SuccessfulNamedHandler).await.expect("subscribe");
    assert_eq!(sub.name(), Some("success-handler"), "Subscription should expose the handler name");
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn subscription_name_returns_none_for_unnamed_handler() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let sub = bus.subscribe(UnnamedSyncHandler).await.expect("subscribe");
    assert_eq!(sub.name(), None, "unnamed handler should yield None");
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn subscription_guard_preserves_name() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let sub = bus.subscribe(SuccessfulNamedHandler).await.expect("subscribe");
    let mut guard = sub.into_guard();
    assert_eq!(guard.name(), Some("success-handler"), "guard should expose the handler name");
    guard.disarm();
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn middleware_subscription_name_is_none() {
    use jaeb::{Middleware, MiddlewareDecision};
    use std::any::Any;

    struct PassThrough;
    impl Middleware for PassThrough {
        async fn process(&self, _event_name: &str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
            MiddlewareDecision::Continue
        }
    }

    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let sub = bus.add_middleware(PassThrough).await.expect("add_middleware");
    assert_eq!(sub.name(), None, "middleware subscriptions should have no name");
    bus.shutdown().await.expect("shutdown");
}
