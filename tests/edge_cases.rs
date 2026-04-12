use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{
    DeadLetter, EventBus, EventBusError, EventHandler, HandlerResult, Middleware, MiddlewareDecision, RetryStrategy, SubscriptionPolicy,
    SyncEventHandler, SyncMiddleware, SyncSubscriptionPolicy,
};

#[derive(Clone, Debug)]
struct EdgeEvent {
    value: usize,
}

#[derive(Clone, Debug)]
struct OtherEvent;

#[derive(Clone, Debug)]
struct RetryEvent;

struct EdgeCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<EdgeEvent> for EdgeCounter {
    fn handle(&self, _event: &EdgeEvent) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct OtherCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<OtherEvent> for OtherCounter {
    fn handle(&self, _event: &OtherEvent) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct PanicSyncMiddleware;

impl SyncMiddleware for PanicSyncMiddleware {
    fn process(&self, _event_name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        panic!("sync middleware panic");
    }
}

struct PanicAsyncMiddleware;

impl Middleware for PanicAsyncMiddleware {
    async fn process(&self, _event_name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        panic!("async middleware panic");
    }
}

struct HaltOnceMiddleware {
    seen: Arc<AtomicBool>,
}

impl SyncMiddleware for HaltOnceMiddleware {
    fn process(&self, _event_name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        if self.seen.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            MiddlewareDecision::Reject("first blocked".to_string())
        } else {
            MiddlewareDecision::Continue
        }
    }
}

struct FailNTimes {
    attempts: Arc<AtomicUsize>,
    fail_count: usize,
}

impl EventHandler<RetryEvent> for FailNTimes {
    async fn handle(&self, _event: &RetryEvent) -> HandlerResult {
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            Err(format!("failure attempt {n}").into())
        } else {
            Ok(())
        }
    }
}

struct PanicOnSecondAttempt {
    attempts: Arc<AtomicUsize>,
}

impl EventHandler<RetryEvent> for PanicOnSecondAttempt {
    async fn handle(&self, _event: &RetryEvent) -> HandlerResult {
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Err("first failure".into())
        } else {
            panic!("panic on second attempt");
        }
    }
}

struct DeadLetterCollector {
    count: Arc<AtomicUsize>,
    attempts: Arc<AtomicUsize>,
    saw_handler_name: Arc<AtomicBool>,
}

impl SyncEventHandler<DeadLetter> for DeadLetterCollector {
    fn handle(&self, event: &DeadLetter) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.attempts.store(event.attempts, Ordering::SeqCst);
        if event.handler_name.is_some() {
            self.saw_handler_name.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

struct PanicDeadLetterHandler;

impl SyncEventHandler<DeadLetter> for PanicDeadLetterHandler {
    fn handle(&self, _event: &DeadLetter) -> HandlerResult {
        panic!("dead letter handler panic");
    }
}

struct SpawnSubscribeDuringDispatch {
    bus: EventBus,
    new_hits: Arc<AtomicUsize>,
    subscribed_tx: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl SyncEventHandler<EdgeEvent> for SpawnSubscribeDuringDispatch {
    fn handle(&self, _event: &EdgeEvent) -> HandlerResult {
        let bus = self.bus.clone();
        let new_hits = Arc::clone(&self.new_hits);
        let subscribed_tx = self.subscribed_tx.lock().expect("subscribed_tx lock").take();

        if let Some(subscribed_tx) = subscribed_tx {
            tokio::runtime::Handle::current().spawn(async move {
                let _sub = bus
                    .subscribe::<EdgeEvent, _, _>(move |_event: &EdgeEvent| {
                        new_hits.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
                    .expect("subscribe from handler");
                let _ = subscribed_tx.send(());
            });
        }

        Ok(())
    }
}

#[tokio::test]
async fn middleware_panic_is_caught() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _mw = bus.add_sync_middleware(PanicSyncMiddleware).await.expect("add middleware");
    let _sub = bus
        .subscribe::<EdgeEvent, _, _>(EdgeCounter { count: Arc::clone(&count) })
        .await
        .expect("subscribe");

    let err = bus.publish(EdgeEvent { value: 1 }).await.expect_err("publish should fail");
    assert!(matches!(
        err,
        EventBusError::MiddlewareRejected(ref reason) if reason.contains("middleware panicked")
    ));

    assert_eq!(count.load(Ordering::SeqCst), 0);
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn async_middleware_panic_is_caught() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _mw = bus.add_middleware(PanicAsyncMiddleware).await.expect("add middleware");
    let _sub = bus
        .subscribe::<EdgeEvent, _, _>(EdgeCounter { count: Arc::clone(&count) })
        .await
        .expect("subscribe");

    let err = bus.publish(EdgeEvent { value: 1 }).await.expect_err("publish should fail");
    assert!(matches!(
        err,
        EventBusError::MiddlewareRejected(ref reason) if reason.contains("middleware panicked")
    ));

    assert_eq!(count.load(Ordering::SeqCst), 0);
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn middleware_plus_once_handler() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let middleware_hits = Arc::new(AtomicUsize::new(0));
    let handler_hits = Arc::new(AtomicUsize::new(0));

    struct CountingMiddleware(Arc<AtomicUsize>);
    impl SyncMiddleware for CountingMiddleware {
        fn process(&self, _event_name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
            self.0.fetch_add(1, Ordering::SeqCst);
            MiddlewareDecision::Continue
        }
    }

    let _mw = bus
        .add_sync_middleware(CountingMiddleware(Arc::clone(&middleware_hits)))
        .await
        .expect("add middleware");

    let handler_hits_for_closure = Arc::clone(&handler_hits);
    let _once = bus
        .subscribe_once::<EdgeEvent, _, _>(move |_event: &EdgeEvent| {
            handler_hits_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe once");

    bus.publish(EdgeEvent { value: 1 }).await.expect("publish 1");
    bus.publish(EdgeEvent { value: 2 }).await.expect("publish 2");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(middleware_hits.load(Ordering::SeqCst), 2);
    assert_eq!(handler_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dead_letter_from_retry_exhaustion() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");

    let attempts = Arc::new(AtomicUsize::new(0));
    let dead_letters = Arc::new(AtomicUsize::new(0));
    let dl_attempts = Arc::new(AtomicUsize::new(0));
    let saw_handler_name = Arc::new(AtomicBool::new(false));

    let _dl = bus
        .subscribe_dead_letters(DeadLetterCollector {
            count: Arc::clone(&dead_letters),
            attempts: Arc::clone(&dl_attempts),
            saw_handler_name: Arc::clone(&saw_handler_name),
        })
        .await
        .expect("subscribe dead letters");

    let policy = SubscriptionPolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(1)))
        .with_dead_letter(true);

    let _sub = bus
        .subscribe_with_policy::<RetryEvent, _, _>(
            FailNTimes {
                attempts: Arc::clone(&attempts),
                fail_count: 10,
            },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(RetryEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(dead_letters.load(Ordering::SeqCst), 1);
    assert_eq!(dl_attempts.load(Ordering::SeqCst), 3);
    assert!(!saw_handler_name.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dead_letter_during_shutdown_drain() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");

    let dead_letters = Arc::new(AtomicUsize::new(0));
    let dl_attempts = Arc::new(AtomicUsize::new(0));
    let saw_handler_name = Arc::new(AtomicBool::new(false));

    let _dl = bus
        .subscribe_dead_letters(DeadLetterCollector {
            count: Arc::clone(&dead_letters),
            attempts: Arc::clone(&dl_attempts),
            saw_handler_name: Arc::clone(&saw_handler_name),
        })
        .await
        .expect("subscribe dead letters");

    let _sub = bus
        .subscribe_with_policy::<RetryEvent, _, _>(
            |_event: RetryEvent| async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Err::<(), _>("shutdown-drain failure".into())
            },
            SubscriptionPolicy::default().with_dead_letter(true),
        )
        .await
        .expect("subscribe");

    bus.publish(RetryEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(dead_letters.load(Ordering::SeqCst), 1);
    assert_eq!(dl_attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_handler_during_shutdown_times_out_cleanly() {
    let bus = EventBus::builder()
        .buffer_size(64)
        .shutdown_timeout(Duration::from_millis(30))
        .build()
        .await
        .expect("valid config");

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_handler = Arc::clone(&attempts);

    let policy = SubscriptionPolicy::default()
        .with_max_retries(3)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(200)))
        .with_dead_letter(false);

    let _sub = bus
        .subscribe_with_policy::<RetryEvent, _, _>(
            move |_event: RetryEvent| {
                let attempts_for_handler = Arc::clone(&attempts_for_handler);
                async move {
                    attempts_for_handler.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>("keep failing".into())
                }
            },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(RetryEvent).await.expect("publish");

    let result = bus.shutdown().await;
    assert_eq!(result, Err(EventBusError::ShutdownTimeout));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn subscribe_during_active_dispatch_uses_next_snapshot() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let initial_hits = Arc::new(AtomicUsize::new(0));
    let new_hits = Arc::new(AtomicUsize::new(0));
    let (subscribed_tx, subscribed_rx) = tokio::sync::oneshot::channel();

    let initial_hits_for_handler = Arc::clone(&initial_hits);
    let _existing = bus
        .subscribe::<EdgeEvent, _, _>(move |_event: &EdgeEvent| {
            initial_hits_for_handler.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe existing");

    let _mutator = bus
        .subscribe_with_policy::<EdgeEvent, _, _>(
            SpawnSubscribeDuringDispatch {
                bus: bus.clone(),
                new_hits: Arc::clone(&new_hits),
                subscribed_tx: Arc::new(std::sync::Mutex::new(Some(subscribed_tx))),
            },
            SyncSubscriptionPolicy::default(),
        )
        .await
        .expect("subscribe mutator handler");

    bus.publish(EdgeEvent { value: 1 }).await.expect("publish 1");

    tokio::time::timeout(Duration::from_secs(1), subscribed_rx)
        .await
        .expect("dynamic subscribe should complete")
        .expect("subscribe completion signal should be sent");

    assert_eq!(new_hits.load(Ordering::SeqCst), 0, "new listener should not receive current event");

    bus.publish(EdgeEvent { value: 2 }).await.expect("publish 2");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(initial_hits.load(Ordering::SeqCst), 2);
    assert_eq!(new_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn handler_panic_during_retry_is_reported() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");

    let attempts = Arc::new(AtomicUsize::new(0));
    let dead_letters = Arc::new(AtomicUsize::new(0));
    let dl_attempts = Arc::new(AtomicUsize::new(0));
    let saw_handler_name = Arc::new(AtomicBool::new(false));

    let _dl = bus
        .subscribe_dead_letters(DeadLetterCollector {
            count: Arc::clone(&dead_letters),
            attempts: Arc::clone(&dl_attempts),
            saw_handler_name: Arc::clone(&saw_handler_name),
        })
        .await
        .expect("subscribe dead letters");

    let policy = SubscriptionPolicy::default().with_max_retries(1).with_dead_letter(true);
    let _sub = bus
        .subscribe_with_policy::<RetryEvent, _, _>(
            PanicOnSecondAttempt {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(RetryEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(dead_letters.load(Ordering::SeqCst), 1);
    assert_eq!(dl_attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn dead_letter_listener_panic_does_not_recurse() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");

    let _failing_dead_letter = bus.subscribe_dead_letters(PanicDeadLetterHandler).await.expect("subscribe dead letters");

    let _failing_listener = bus
        .subscribe::<EdgeEvent, _, _>(|_event: &EdgeEvent| Err::<(), _>("primary failure".into()))
        .await
        .expect("subscribe failing listener");

    bus.publish(EdgeEvent { value: 1 }).await.expect("publish");
    tokio::time::timeout(Duration::from_secs(2), bus.shutdown())
        .await
        .expect("shutdown timed out")
        .expect("shutdown");
}

#[tokio::test]
async fn zero_max_retries_means_no_retry() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    let policy = SubscriptionPolicy::default().with_max_retries(0).with_dead_letter(false);
    let _sub = bus
        .subscribe_with_policy::<RetryEvent, _, _>(
            FailNTimes {
                attempts: Arc::clone(&attempts),
                fail_count: usize::MAX,
            },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(RetryEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn publish_after_all_listeners_unsubscribed() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus
        .subscribe::<EdgeEvent, _, _>(EdgeCounter { count: Arc::clone(&count) })
        .await
        .expect("subscribe");

    sub.unsubscribe().await.expect("unsubscribe");
    bus.publish(EdgeEvent { value: 1 }).await.expect("publish with no listeners");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn middleware_rejects_then_accepts() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _mw = bus
        .add_sync_middleware(HaltOnceMiddleware {
            seen: Arc::new(AtomicBool::new(false)),
        })
        .await
        .expect("add middleware");

    let _sub = bus
        .subscribe::<EdgeEvent, _, _>(EdgeCounter { count: Arc::clone(&count) })
        .await
        .expect("subscribe");

    let first = bus.publish(EdgeEvent { value: 1 }).await;
    assert!(matches!(first, Err(EventBusError::MiddlewareRejected(_))));

    bus.publish(EdgeEvent { value: 2 }).await.expect("second publish should pass");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn builder_default_values_are_sane() {
    let bus = EventBus::builder().build().await.expect("default builder should build");
    let stats = bus.stats().await.expect("stats");

    assert_eq!(stats.queue_capacity, 256);
    assert_eq!(stats.publish_permits_available, 256);
    assert_eq!(stats.publish_in_flight, 0);
    assert_eq!(stats.total_subscriptions, 0);
    assert!(stats.registered_event_types.is_empty());

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn try_publish_on_full_channel() {
    let bus = EventBus::builder().buffer_size(1).build().await.expect("valid config");

    let _slow = bus
        .subscribe::<EdgeEvent, _, _>(|event: EdgeEvent| async move {
            let _ = event.value;
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(())
        })
        .await
        .expect("subscribe");

    bus.try_publish(EdgeEvent { value: 1 }).expect("first try_publish succeeds");
    let err = bus.try_publish(EdgeEvent { value: 2 }).expect_err("second try_publish should fail");
    assert_eq!(err, EventBusError::ChannelFull);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn multiple_event_types_independent() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");

    let edge_count = Arc::new(AtomicUsize::new(0));
    let other_count = Arc::new(AtomicUsize::new(0));

    let _edge = bus
        .subscribe::<EdgeEvent, _, _>(EdgeCounter {
            count: Arc::clone(&edge_count),
        })
        .await
        .expect("subscribe edge");

    let _other = bus
        .subscribe::<OtherEvent, _, _>(OtherCounter {
            count: Arc::clone(&other_count),
        })
        .await
        .expect("subscribe other");

    bus.publish(EdgeEvent { value: 1 }).await.expect("publish edge");
    assert_eq!(edge_count.load(Ordering::SeqCst), 1);
    assert_eq!(other_count.load(Ordering::SeqCst), 0);

    bus.publish(OtherEvent).await.expect("publish other");
    assert_eq!(edge_count.load(Ordering::SeqCst), 1);
    assert_eq!(other_count.load(Ordering::SeqCst), 1);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn subscription_id_uniqueness_under_concurrency() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
    let ids = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..100 {
        let bus_cloned = bus.clone();
        let ids_cloned = Arc::clone(&ids);
        tasks.spawn(async move {
            let sub = bus_cloned
                .subscribe::<EdgeEvent, _, _>(|_event: &EdgeEvent| Ok(()))
                .await
                .expect("subscribe");
            ids_cloned.lock().await.push(sub.id().as_u64());
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.expect("task should not panic");
    }

    let mut values = ids.lock().await.clone();
    values.sort_unstable();
    values.dedup();
    assert_eq!(values.len(), 100);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn concurrent_stats_access_is_safe() {
    let bus = EventBus::builder().buffer_size(128).build().await.expect("valid config");
    let total = Arc::new(AtomicUsize::new(0));

    let total_for_handler = Arc::clone(&total);
    let _sub = bus
        .subscribe::<EdgeEvent, _, _>(move |_event: &EdgeEvent| {
            total_for_handler.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe");

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let bus_cloned = bus.clone();
        tasks.spawn(async move {
            for _ in 0..25 {
                let stats = bus_cloned.stats().await.expect("stats");
                assert!(stats.publish_permits_available <= stats.queue_capacity);
            }
        });
    }

    for _ in 0..250 {
        bus.publish(EdgeEvent { value: 1 }).await.expect("publish");
    }

    while let Some(result) = tasks.join_next().await {
        result.expect("stats task should not panic");
    }

    bus.shutdown().await.expect("shutdown");
    assert_eq!(total.load(Ordering::SeqCst), 250);
}
