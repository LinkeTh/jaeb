use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{DeadLetter, EventBus, EventBusError, EventHandler, FailurePolicy, HandlerResult, SyncEventHandler};
use tokio::sync::Notify;

#[derive(Clone, Debug)]
struct NumberEvent {
    value: usize,
}

struct SumSyncHandler {
    sum: Arc<AtomicUsize>,
}

impl SyncEventHandler<NumberEvent> for SumSyncHandler {
    fn handle(&self, event: &NumberEvent) -> HandlerResult {
        self.sum.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

struct RetryOnceAsyncHandler {
    attempts: Arc<AtomicUsize>,
}

impl EventHandler<NumberEvent> for RetryOnceAsyncHandler {
    async fn handle(&self, _event: &NumberEvent) -> HandlerResult {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 { Err("first attempt fails".into()) } else { Ok(()) }
    }
}

struct AlwaysFailSyncHandler;

impl SyncEventHandler<NumberEvent> for AlwaysFailSyncHandler {
    fn handle(&self, _event: &NumberEvent) -> HandlerResult {
        Err("always fails".into())
    }
}

#[tokio::test]
async fn sync_handler_receives_event() {
    let bus = EventBus::new(16);
    let sum = Arc::new(AtomicUsize::new(0));

    bus.register_sync::<NumberEvent, _>(SumSyncHandler { sum: Arc::clone(&sum) })
        .await
        .expect("failed to register sync handler");

    bus.publish(NumberEvent { value: 7 }).await.expect("failed to publish event");

    assert_eq!(sum.load(Ordering::SeqCst), 7);
    bus.shutdown().await.expect("failed to shutdown bus");
}

#[tokio::test]
async fn unsubscribe_stops_delivery() {
    let bus = EventBus::new(16);
    let sum = Arc::new(AtomicUsize::new(0));

    let subscription = bus
        .register_sync::<NumberEvent, _>(SumSyncHandler { sum: Arc::clone(&sum) })
        .await
        .expect("failed to register sync handler");

    bus.publish(NumberEvent { value: 2 }).await.expect("failed to publish event");

    subscription.unsubscribe().await.expect("unsubscribe failed");

    bus.publish(NumberEvent { value: 3 }).await.expect("failed to publish event");

    assert_eq!(sum.load(Ordering::SeqCst), 2);
    bus.shutdown().await.expect("failed to shutdown bus");
}

#[tokio::test]
async fn retry_policy_retries_failures() {
    let bus = EventBus::new(16);
    let attempts = Arc::new(AtomicUsize::new(0));

    let policy = FailurePolicy::default().with_max_retries(1).with_retry_delay(Duration::from_millis(1));

    bus.register_with_policy::<NumberEvent, _>(
        RetryOnceAsyncHandler {
            attempts: Arc::clone(&attempts),
        },
        policy,
    )
    .await
    .expect("failed to register async handler");

    bus.publish(NumberEvent { value: 1 }).await.expect("failed to publish event");

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    bus.shutdown().await.expect("failed to shutdown bus");
}

#[tokio::test]
async fn failed_handler_emits_dead_letter() {
    let bus = EventBus::new(16);
    let notify = Arc::new(Notify::new());
    let seen_dead_letters = Arc::new(AtomicUsize::new(0));

    let notify_clone = Arc::clone(&notify);
    let seen_clone = Arc::clone(&seen_dead_letters);
    bus.subscribe_dead_letters(move |_dl: &DeadLetter| {
        seen_clone.fetch_add(1, Ordering::SeqCst);
        notify_clone.notify_one();
        Ok(())
    })
    .await
    .expect("failed to subscribe dead letters");

    bus.register_sync::<NumberEvent, _>(AlwaysFailSyncHandler)
        .await
        .expect("failed to register failing handler");

    bus.publish(NumberEvent { value: 1 }).await.expect("failed to publish event");

    tokio::time::timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("timed out waiting for dead letter");

    assert_eq!(seen_dead_letters.load(Ordering::SeqCst), 1);
    bus.shutdown().await.expect("failed to shutdown bus");
}

#[tokio::test]
async fn try_publish_reports_channel_full() {
    let bus = EventBus::new(1);

    bus.try_publish(NumberEvent { value: 1 }).expect("first try_publish should enqueue");

    let err = bus
        .try_publish(NumberEvent { value: 2 })
        .expect_err("second try_publish should fail when queue is full");

    assert_eq!(err, EventBusError::ChannelFull);
    bus.shutdown().await.expect("failed to shutdown bus");
}

#[tokio::test]
async fn shutdown_stops_new_operations() {
    let bus = EventBus::new(16);
    bus.shutdown().await.expect("failed to shutdown bus");

    let subscribe_err = bus
        .subscribe_sync::<NumberEvent, _>(|_event| Ok(()))
        .await
        .expect_err("subscribe should fail after shutdown");
    assert_eq!(subscribe_err, EventBusError::ActorStopped);

    let publish_err = bus
        .publish(NumberEvent { value: 1 })
        .await
        .expect_err("publish should fail after shutdown");
    assert_eq!(publish_err, EventBusError::ActorStopped);
}
