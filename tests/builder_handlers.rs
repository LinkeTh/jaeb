use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use jaeb::{
    DeadLetter, DeadLetterDescriptor, Deps, EventBus, EventBusError, EventHandler, HandlerDescriptor, HandlerResult, Subscription, SyncEventHandler,
};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TestEvent {
    #[allow(dead_code)]
    value: u32,
}

// ── Shared dep ───────────────────────────────────────────────────────

struct Counter(Arc<AtomicUsize>);

// ── Async handler with no deps ───────────────────────────────────────

struct NoDepsHandler {
    fired: Arc<AtomicBool>,
}

impl EventHandler<TestEvent> for NoDepsHandler {
    async fn handle(&self, _event: &TestEvent) -> HandlerResult {
        self.fired.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl HandlerDescriptor for NoDepsHandler {
    fn register<'a>(
        &'a self,
        bus: &'a EventBus,
        _deps: &'a Deps,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
        Box::pin(async move {
            bus.subscribe::<TestEvent, _, _>(NoDepsHandler {
                fired: Arc::clone(&self.fired),
            })
            .await
        })
    }
}

// ── Async handler that resolves a dep ────────────────────────────────

struct WithDepHandler;

impl EventHandler<TestEvent> for WithDepHandler {
    async fn handle(&self, _event: &TestEvent) -> HandlerResult {
        Ok(())
    }
}

struct WithDepDescriptor {
    seen: Arc<AtomicUsize>,
}

impl HandlerDescriptor for WithDepDescriptor {
    fn register<'a>(
        &'a self,
        bus: &'a EventBus,
        deps: &'a Deps,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
        let counter = deps.get::<Counter>().map(|c| Arc::clone(&c.0));
        let seen = Arc::clone(&self.seen);
        Box::pin(async move {
            // Record that we found (or didn't find) the dep at build time.
            if let Some(c) = counter {
                seen.store(c.load(Ordering::SeqCst), Ordering::SeqCst);
            }
            bus.subscribe::<TestEvent, _, _>(WithDepHandler).await
        })
    }
}

// ── Descriptor that requires a dep (fails without it) ────────────────

struct RequiredDepDescriptor;

impl HandlerDescriptor for RequiredDepDescriptor {
    fn register<'a>(
        &'a self,
        _bus: &'a EventBus,
        deps: &'a Deps,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
        // Try to get a dep that is never inserted in relevant tests.
        struct Missing;
        Box::pin(async move {
            let _dep = deps.get_required::<Missing>()?;
            unreachable!()
        })
    }
}

// ── Dead-letter handler ───────────────────────────────────────────────

struct LogDeadLetters {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<DeadLetter> for LogDeadLetters {
    fn handle(&self, _dl: &DeadLetter) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl DeadLetterDescriptor for LogDeadLetters {
    fn register_dead_letter<'a>(
        &'a self,
        bus: &'a EventBus,
        _deps: &'a Deps,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
        Box::pin(async move {
            bus.subscribe_dead_letters(LogDeadLetters {
                count: Arc::clone(&self.count),
            })
            .await
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

/// A builder with no handlers builds successfully and the bus works.
#[tokio::test]
async fn build_with_no_handlers_succeeds() {
    let bus = EventBus::builder().build().await.expect("build should succeed");
    // Basic sanity: publish an event with no subscribers is fine.
    bus.publish(TestEvent { value: 1 }).await.expect("publish");
}

/// A handler registered via `.handler()` is active after build.
#[tokio::test]
async fn handler_registered_via_builder_receives_event() {
    let fired = Arc::new(AtomicBool::new(false));

    let bus = EventBus::builder()
        .buffer_size(16)
        .handler(NoDepsHandler { fired: Arc::clone(&fired) })
        .build()
        .await
        .expect("build should succeed");

    bus.publish(TestEvent { value: 1 }).await.expect("publish");
    // Give the async handler time to run.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(fired.load(Ordering::SeqCst), "handler should have fired");
}

/// Deps inserted via `.deps()` are available to the descriptor at build time.
#[tokio::test]
async fn handler_descriptor_can_read_deps() {
    let counter = Arc::new(AtomicUsize::new(42));
    let seen = Arc::new(AtomicUsize::new(0));

    let _bus = EventBus::builder()
        .handler(WithDepDescriptor { seen: Arc::clone(&seen) })
        .deps(Deps::new().insert(Counter(Arc::clone(&counter))))
        .build()
        .await
        .expect("build should succeed");

    assert_eq!(seen.load(Ordering::SeqCst), 42, "descriptor should have read the dep value at build time");
}

/// Missing required dep causes `build()` to return `MissingDependency`.
#[tokio::test]
async fn build_returns_missing_dependency_error() {
    let result = EventBus::builder().handler(RequiredDepDescriptor).build().await;

    match result {
        Err(EventBusError::MissingDependency(name)) => {
            assert!(name.contains("Missing"), "expected type name in error, got: {name}");
        }
        other => panic!("expected MissingDependency error, got: {other:?}"),
    }
}

/// Multiple handlers can be chained on the builder.
#[tokio::test]
async fn multiple_handlers_all_registered() {
    let fired_a = Arc::new(AtomicBool::new(false));
    let fired_b = Arc::new(AtomicBool::new(false));

    let bus = EventBus::builder()
        .handler(NoDepsHandler { fired: Arc::clone(&fired_a) })
        .handler(NoDepsHandler { fired: Arc::clone(&fired_b) })
        .build()
        .await
        .expect("build should succeed");

    bus.publish(TestEvent { value: 0 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(fired_a.load(Ordering::SeqCst), "handler A should have fired");
    assert!(fired_b.load(Ordering::SeqCst), "handler B should have fired");
}

/// A dead-letter handler registered via `.dead_letter()` is invoked after a
/// terminal handler failure.
#[tokio::test]
async fn dead_letter_handler_registered_via_builder() {
    use jaeb::{HandlerError, SubscriptionPolicy};

    struct AlwaysFails;

    impl EventHandler<TestEvent> for AlwaysFails {
        async fn handle(&self, _event: &TestEvent) -> HandlerResult {
            Err(HandlerError::from("always fails"))
        }
    }

    struct AlwaysFailsDescriptor;

    impl HandlerDescriptor for AlwaysFailsDescriptor {
        fn register<'a>(
            &'a self,
            bus: &'a EventBus,
            _deps: &'a Deps,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
            Box::pin(async move {
                bus.subscribe_with_policy::<TestEvent, _, _>(AlwaysFails, SubscriptionPolicy::default().with_max_retries(0).with_dead_letter(true))
                    .await
            })
        }
    }

    let dl_count = Arc::new(AtomicUsize::new(0));

    let bus = EventBus::builder()
        .handler(AlwaysFailsDescriptor)
        .dead_letter(LogDeadLetters {
            count: Arc::clone(&dl_count),
        })
        .build()
        .await
        .expect("build should succeed");

    bus.publish(TestEvent { value: 99 }).await.expect("publish");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert!(dl_count.load(Ordering::SeqCst) > 0, "dead-letter handler should have been invoked");
}

/// Compile-time enforcement: `.handler()` rejects `DeadLetterDescriptor`-only types
/// and `.dead_letter()` rejects `HandlerDescriptor`-only types.
/// These are verified by the trait bounds — no runtime test needed, but we
/// document the contract here for clarity.
#[tokio::test]
async fn builder_api_compiles_with_correct_types() {
    // If this test compiles, the type constraints are satisfied.
    let _ = EventBus::builder()
        .handler(NoDepsHandler {
            fired: Arc::new(AtomicBool::new(false)),
        })
        .dead_letter(LogDeadLetters {
            count: Arc::new(AtomicUsize::new(0)),
        })
        .build()
        .await
        .expect("build should succeed");
}
