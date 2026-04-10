use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use jaeb::{DeadLetter, EventBus, EventBusError, FailurePolicy, HandlerResult, NoRetryPolicy, SyncEventHandler};

#[derive(Clone)]
struct Ping {
    value: usize,
}

struct SyncCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<Ping> for SyncCounter {
    fn handle(&self, event: &Ping) -> HandlerResult {
        self.count.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

struct DeadLetterCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<DeadLetter> for DeadLetterCounter {
    fn handle(&self, _event: &DeadLetter) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn sync_closure_receives_event() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let count_for_handler = Arc::clone(&count);
    let _sub = bus
        .subscribe::<Ping, _, _>(move |event: &Ping| {
            count_for_handler.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe sync closure");

    bus.publish(Ping { value: 3 }).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn async_closure_receives_event() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let count_for_handler = Arc::clone(&count);
    let _sub = bus
        .subscribe::<Ping, _, _>(move |event: Ping| {
            let count_for_handler = Arc::clone(&count_for_handler);
            async move {
                count_for_handler.fetch_add(event.value, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .expect("subscribe async closure");

    bus.publish(Ping { value: 5 }).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn closure_with_failure_policy_emits_dead_letter() {
    let bus = EventBus::new(64).expect("valid config");
    let dead_letters = Arc::new(AtomicUsize::new(0));

    let _dl = bus
        .subscribe_dead_letters(DeadLetterCounter {
            count: Arc::clone(&dead_letters),
        })
        .await
        .expect("subscribe dead letters");

    let policy = FailurePolicy::default().with_max_retries(1);
    let _sub = bus
        .subscribe_with_policy::<Ping, _, _>(|_event: Ping| async move { Err::<(), _>("closure failed".into()) }, policy)
        .await
        .expect("subscribe closure with policy");

    bus.publish(Ping { value: 1 }).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(dead_letters.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn closure_once_listener_fires_once() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let count_for_handler = Arc::clone(&count);
    let _sub = bus
        .subscribe_once_with_policy::<Ping, _, _>(
            move |event: &Ping| {
                count_for_handler.fetch_add(event.value, Ordering::SeqCst);
                Ok(())
            },
            NoRetryPolicy::default(),
        )
        .await
        .expect("subscribe once closure");

    bus.publish(Ping { value: 1 }).await.expect("publish one");
    bus.publish(Ping { value: 1 }).await.expect("publish two");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 1, "once closure should fire exactly once");
}

#[tokio::test]
async fn multiple_closure_handlers_same_event_type() {
    let bus = EventBus::new(64).expect("valid config");
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));

    let a_for_handler = Arc::clone(&a);
    let _sub_a = bus
        .subscribe::<Ping, _, _>(move |event: &Ping| {
            a_for_handler.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe closure a");

    let b_for_handler = Arc::clone(&b);
    let _sub_b = bus
        .subscribe::<Ping, _, _>(move |event: &Ping| {
            b_for_handler.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe closure b");

    bus.publish(Ping { value: 2 }).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(a.load(Ordering::SeqCst), 2);
    assert_eq!(b.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn closure_and_struct_handler_same_event_type() {
    let bus = EventBus::new(64).expect("valid config");
    let struct_count = Arc::new(AtomicUsize::new(0));
    let closure_count = Arc::new(AtomicUsize::new(0));

    let _struct_sub = bus
        .subscribe::<Ping, _, _>(SyncCounter {
            count: Arc::clone(&struct_count),
        })
        .await
        .expect("subscribe struct handler");

    let closure_count_for_handler = Arc::clone(&closure_count);
    let _closure_sub = bus
        .subscribe::<Ping, _, _>(move |event: &Ping| {
            closure_count_for_handler.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe closure handler");

    bus.publish(Ping { value: 4 }).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(struct_count.load(Ordering::SeqCst), 4);
    assert_eq!(closure_count.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn typed_middleware_rejects_closure_handler() {
    struct RejectPing;

    impl jaeb::TypedSyncMiddleware<Ping> for RejectPing {
        fn process(&self, _event_name: &'static str, _event: &Ping) -> jaeb::MiddlewareDecision {
            jaeb::MiddlewareDecision::Reject("blocked ping".to_string())
        }
    }

    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _typed = bus.add_typed_sync_middleware::<Ping, _>(RejectPing).await.expect("add typed middleware");

    let count_for_handler = Arc::clone(&count);
    let _sub = bus
        .subscribe::<Ping, _, _>(move |event: &Ping| {
            count_for_handler.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe closure");

    let err = bus.publish(Ping { value: 1 }).await.unwrap_err();
    assert!(matches!(err, EventBusError::MiddlewareRejected(ref r) if r == "blocked ping"));

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 0, "handler should not run after typed middleware rejection");
}
