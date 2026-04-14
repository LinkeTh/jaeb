//! Integration tests for `#[handler]` and `#[dead_letter_handler]` with
//! `Dep<T>` parameters.
//!
//! Each test verifies that the macro-generated code correctly resolves
//! dependencies at build time and passes them to the handler function.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{AsyncSubscriptionPolicy, DeadLetter, Dep, Deps, EventBus, EventBusError, HandlerError, HandlerResult, dead_letter_handler, handler};

// ── Event types ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct OrderEvent {
    #[allow(dead_code)]
    id: u32,
}

// ── Shared counter dep ────────────────────────────────────────────────────────

#[derive(Clone)]
struct Counter(Arc<AtomicUsize>);

impl Counter {
    fn inc(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

// ── Async handler with a single Dep<T> (destructured form) ───────────────────

/// async handler receives an `Arc<Counter>` via `Dep(counter): Dep<Counter>`
#[handler]
async fn async_with_dep(event: &OrderEvent, Dep(counter): Dep<Counter>) -> HandlerResult {
    let _ = event;
    counter.inc();
    Ok(())
}

#[tokio::test]
async fn async_handler_with_dep_receives_and_uses_dep() {
    let counter = Counter(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .handler(async_with_dep)
        .deps(Deps::new().insert(counter.clone()))
        .build()
        .await
        .expect("build should succeed");

    bus.publish(OrderEvent { id: 1 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(counter.get(), 1, "async handler with dep should have incremented the counter");
}

// ── Sync handler with a single Dep<T> (plain identifier form) ────────────────

/// sync handler receives a `Counter` via `counter: Dep<Counter>`
#[handler]
fn sync_with_dep(event: &OrderEvent, counter: Dep<Counter>) -> HandlerResult {
    let _ = event;
    counter.0.inc();
    Ok(())
}

#[tokio::test]
async fn sync_handler_with_dep_receives_and_uses_dep() {
    let counter = Counter(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .handler(sync_with_dep)
        .deps(Deps::new().insert(counter.clone()))
        .build()
        .await
        .expect("build should succeed");

    bus.publish(OrderEvent { id: 2 }).await.expect("publish");
    // Sync handlers complete before publish returns, so no sleep needed.

    assert_eq!(counter.get(), 1, "sync handler with dep should have incremented the counter");
}

// ── Multiple Dep<T> params ────────────────────────────────────────────────────

#[derive(Clone)]
struct EventLog(Arc<AtomicUsize>);

#[handler]
async fn multi_dep_handler(_event: &OrderEvent, Dep(counter): Dep<Counter>, Dep(log): Dep<EventLog>) -> HandlerResult {
    counter.inc();
    log.0.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn handler_with_multiple_deps_resolves_all() {
    let counter = Counter(Arc::new(AtomicUsize::new(0)));
    let log = EventLog(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .handler(multi_dep_handler)
        .deps(Deps::new().insert(counter.clone()).insert(log.clone()))
        .build()
        .await
        .expect("build should succeed");

    bus.publish(OrderEvent { id: 3 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(counter.get(), 1, "counter dep should be incremented");
    assert_eq!(log.0.load(Ordering::SeqCst), 1, "log dep should be incremented");
}

// ── Missing dep causes build() error ─────────────────────────────────────────

#[derive(Clone)]
struct MissingDep;

#[handler]
async fn handler_needing_missing_dep(_event: &OrderEvent, _dep: Dep<MissingDep>) -> HandlerResult {
    Ok(())
}

#[tokio::test]
async fn missing_dep_returns_error_at_build_time() {
    let result = EventBus::builder()
        .handler(handler_needing_missing_dep)
        // intentionally NOT inserting MissingDep
        .build()
        .await;

    match result {
        Err(EventBusError::MissingDependency(name)) => {
            assert!(name.contains("MissingDep"), "expected MissingDep in error, got: {name}");
        }
        other => panic!("expected MissingDependency, got: {other:?}"),
    }
}

// ── Dep handler with subscription policy ─────────────────────────────────────

#[handler(retries = 1, dead_letter = true)]
async fn failing_dep_handler(_event: &OrderEvent, Dep(counter): Dep<Counter>) -> HandlerResult {
    counter.inc();
    Err(HandlerError::from("intentional failure"))
}

#[dead_letter_handler]
fn dl_catcher(event: &DeadLetter) -> HandlerResult {
    let _ = event;
    Ok(())
}

#[tokio::test]
async fn dep_handler_with_policy_retries_and_emits_dead_letter() {
    let counter = Counter(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .handler(failing_dep_handler)
        .dead_letter(dl_catcher)
        .deps(Deps::new().insert(counter.clone()))
        .build()
        .await
        .expect("build should succeed");

    bus.publish(OrderEvent { id: 4 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // retries = 1 → 1 initial attempt + 1 retry = 2 total calls
    assert_eq!(counter.get(), 2, "handler with retries=1 should be called twice (initial + 1 retry)");
}

// ── dead_letter_handler with Dep<T> ──────────────────────────────────────────

#[dead_letter_handler]
fn dl_with_dep(_event: &DeadLetter, Dep(counter): Dep<Counter>) -> HandlerResult {
    counter.inc();
    Ok(())
}

struct AlwaysFails;

impl jaeb::EventHandler<OrderEvent> for AlwaysFails {
    async fn handle(&self, _event: &OrderEvent, _bus: &EventBus) -> HandlerResult {
        Err(HandlerError::from("always fails"))
    }
}

impl jaeb::HandlerDescriptor for AlwaysFails {
    fn register<'a>(
        &'a self,
        bus: &'a EventBus,
        _deps: &'a Deps,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<jaeb::Subscription, EventBusError>> + Send + 'a>> {
        Box::pin(async move {
            bus.subscribe_with_policy::<OrderEvent, _, _>(AlwaysFails, AsyncSubscriptionPolicy::default().with_max_retries(0).with_dead_letter(true))
                .await
        })
    }
}

#[tokio::test]
async fn dead_letter_handler_with_dep_is_invoked() {
    let counter = Counter(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .handler(AlwaysFails)
        .dead_letter(dl_with_dep)
        .deps(Deps::new().insert(counter.clone()))
        .build()
        .await
        .expect("build should succeed");

    bus.publish(OrderEvent { id: 5 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(counter.get(), 1, "dead-letter handler with dep should have been invoked once");
}

// ── No-dep handler path is unchanged ─────────────────────────────────────────

#[handler]
async fn no_dep_handler(_event: &OrderEvent) -> HandlerResult {
    Ok(())
}

#[tokio::test]
async fn no_dep_handler_still_works() {
    let bus = EventBus::builder().handler(no_dep_handler).build().await.expect("build should succeed");

    bus.publish(OrderEvent { id: 6 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
}

// ── Sync handler with multiple deps ──────────────────────────────────────────

#[handler]
fn sync_multi_dep(_event: &OrderEvent, Dep(counter): Dep<Counter>, Dep(log): Dep<EventLog>) -> HandlerResult {
    counter.inc();
    log.0.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn sync_handler_with_multiple_deps_resolves_all() {
    let counter = Counter(Arc::new(AtomicUsize::new(0)));
    let log = EventLog(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .handler(sync_multi_dep)
        .deps(Deps::new().insert(counter.clone()).insert(log.clone()))
        .build()
        .await
        .expect("build should succeed");

    bus.publish(OrderEvent { id: 7 }).await.expect("publish");

    assert_eq!(counter.get(), 1, "counter dep should be incremented");
    assert_eq!(log.0.load(Ordering::SeqCst), 1, "log dep should be incremented");
}

// ── Handler with name attr + deps ─────────────────────────────────────────────

#[handler(name = "named-dep-handler")]
async fn named_dep_handler(_event: &OrderEvent, Dep(counter): Dep<Counter>) -> HandlerResult {
    counter.inc();
    Ok(())
}

#[tokio::test]
async fn handler_with_name_attr_and_dep_works() {
    let counter = Counter(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .handler(named_dep_handler)
        .deps(Deps::new().insert(counter.clone()))
        .build()
        .await
        .expect("build should succeed");

    bus.publish(OrderEvent { id: 8 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(counter.get(), 1, "named handler with dep should have fired");
}
