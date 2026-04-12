use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{DeadLetter, EventBus, EventBusError, SubscriptionPolicy, SyncEventHandler, SyncSubscriptionPolicy};
use proptest::prelude::*;

#[derive(Clone, Debug)]
struct PropEvent {
    _value: u16,
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

fn test_config() -> proptest::test_runner::Config {
    proptest::test_runner::Config {
        cases: 200,
        max_shrink_iters: 200,
        ..proptest::test_runner::Config::default()
    }
}

struct DeadLetterCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<DeadLetter> for DeadLetterCounter {
    fn handle(&self, _event: &DeadLetter) -> Result<(), jaeb::HandlerError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

proptest! {
    #![proptest_config(test_config())]

    #[test]
    fn all_sync_listeners_see_every_event(listeners in 1usize..8, events in 1usize..20) {
        rt().block_on(async move {
            let bus = EventBus::builder().buffer_size(256).build().await.expect("valid config");
            let hits = Arc::new(AtomicUsize::new(0));

            for _ in 0..listeners {
                let hits_for_handler = Arc::clone(&hits);
                let _sub = bus
                    .subscribe::<PropEvent, _, _>(move |_event: &PropEvent| {
                        hits_for_handler.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
                    .expect("subscribe");
            }

            for _ in 0..events {
                bus.publish(PropEvent { _value: 1 }).await.expect("publish");
            }

            bus.shutdown().await.expect("shutdown");
            assert_eq!(hits.load(Ordering::SeqCst), listeners * events);
        });
    }

    #[test]
    fn priority_ordering_respected(p1 in -20i32..20, p2 in -20i32..20, p3 in -20i32..20) {
        rt().block_on(async move {
            let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
            let order = Arc::new(Mutex::new(Vec::<usize>::new()));

            let register = |id: usize, priority: i32, order: Arc<Mutex<Vec<usize>>>, bus: EventBus| async move {
                let _sub = bus
                    .subscribe_with_policy::<PropEvent, _, _>(
                        move |_event: &PropEvent| {
                            order.lock().expect("order lock").push(id);
                            Ok(())
                        },
                        SyncSubscriptionPolicy::default().with_priority(priority),
                    )
                    .await
                    .expect("subscribe");
            };

            register(1, p1, Arc::clone(&order), bus.clone()).await;
            register(2, p2, Arc::clone(&order), bus.clone()).await;
            register(3, p3, Arc::clone(&order), bus.clone()).await;

            bus.publish(PropEvent { _value: 1 }).await.expect("publish");
            bus.shutdown().await.expect("shutdown");

            let got = order.lock().expect("order lock").clone();
            assert_eq!(got.len(), 3);

            let mut expected = vec![(1usize, p1), (2usize, p2), (3usize, p3)];
            expected.sort_by(|a, b| b.1.cmp(&a.1));
            let expected_ids: Vec<usize> = expected.into_iter().map(|(id, _)| id).collect();
            assert_eq!(got, expected_ids);
        });
    }

    #[test]
    fn retry_count_bounded(max_retries in 0usize..5) {
        rt().block_on(async move {
            let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
            let attempts = Arc::new(AtomicUsize::new(0));

            let policy = SubscriptionPolicy::default()
                .with_max_retries(max_retries)
                .with_dead_letter(false);

            let attempts_for_handler = Arc::clone(&attempts);
            let _sub = bus
                .subscribe_with_policy::<PropEvent, _, _>(
                    move |_event: PropEvent| {
                        let attempts_for_handler = Arc::clone(&attempts_for_handler);
                        async move {
                            attempts_for_handler.fetch_add(1, Ordering::SeqCst);
                            Err::<(), _>("always fail".into())
                        }
                    },
                    policy,
                )
                .await
                .expect("subscribe");

            bus.publish(PropEvent { _value: 1 }).await.expect("publish");
            bus.shutdown().await.expect("shutdown");

            assert_eq!(attempts.load(Ordering::SeqCst), max_retries + 1);
        });
    }

    #[test]
    fn dead_letter_iff_enabled_and_failed(dead_letter_enabled in any::<bool>(), fail_count in 0usize..4, max_retries in 0usize..4) {
        rt().block_on(async move {
            let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");

            let attempts = Arc::new(AtomicUsize::new(0));
            let dead_letters = Arc::new(AtomicUsize::new(0));

            let dead_letters_for_handler = Arc::clone(&dead_letters);
            let _dl = bus
                .subscribe_dead_letters(DeadLetterCounter {
                    count: dead_letters_for_handler,
                })
                .await
                .expect("subscribe dead letters");

            let policy = SubscriptionPolicy::default()
                .with_max_retries(max_retries)
                .with_dead_letter(dead_letter_enabled);

            let attempts_for_handler = Arc::clone(&attempts);
            let _sub = bus
                .subscribe_with_policy::<PropEvent, _, _>(
                    move |_event: PropEvent| {
                        let attempts_for_handler = Arc::clone(&attempts_for_handler);
                        async move {
                            let n = attempts_for_handler.fetch_add(1, Ordering::SeqCst);
                            if n < fail_count {
                                Err::<(), _>("fail".into())
                            } else {
                                Ok(())
                            }
                        }
                    },
                    policy,
                )
                .await
                .expect("subscribe");

            bus.publish(PropEvent { _value: 1 }).await.expect("publish");
            bus.shutdown().await.expect("shutdown");

            let terminal_failure = fail_count > max_retries;
            let expected_dead_letters = if dead_letter_enabled && terminal_failure { 1 } else { 0 };
            assert_eq!(dead_letters.load(Ordering::SeqCst), expected_dead_letters);
        });
    }

    #[test]
    fn once_handler_fires_at_most_once(events in 1usize..50, values in proptest::collection::vec(0u16..1000, 1..50)) {
        rt().block_on(async move {
            let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
            let hits = Arc::new(AtomicUsize::new(0));

            let hits_for_handler = Arc::clone(&hits);
            let _sub = bus
                .subscribe_once::<PropEvent, _, _>(move |_event: &PropEvent| {
                    hits_for_handler.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .expect("subscribe once");

            for i in 0..events {
                let value = values.get(i % values.len()).copied().unwrap_or(0);
                bus.publish(PropEvent { _value: value }).await.expect("publish");
            }

            bus.shutdown().await.expect("shutdown");
            assert!(hits.load(Ordering::SeqCst) <= 1);
        });
    }

    #[test]
    fn unsubscribe_prevents_future_dispatch(first_batch in 1usize..10, second_batch in 1usize..10) {
        rt().block_on(async move {
            let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
            let hits = Arc::new(AtomicUsize::new(0));

            let hits_for_handler = Arc::clone(&hits);
            let sub = bus
                .subscribe::<PropEvent, _, _>(move |_event: &PropEvent| {
                    hits_for_handler.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .expect("subscribe");

            for _ in 0..first_batch {
                bus.publish(PropEvent { _value: 1 }).await.expect("publish before unsubscribe");
            }

            let removed = sub.unsubscribe().await.expect("unsubscribe");
            assert!(removed, "first unsubscribe should remove the listener");

            for _ in 0..second_batch {
                bus.publish(PropEvent { _value: 2 }).await.expect("publish after unsubscribe");
            }

            bus.shutdown().await.expect("shutdown");
            assert_eq!(hits.load(Ordering::SeqCst), first_batch);
        });
    }
}

#[test]
fn prop_event_payload_roundtrip_sanity() {
    let event = PropEvent { _value: 42 };
    assert_eq!(event._value, 42);
}

#[tokio::test]
async fn unsubscribe_returns_false_when_already_removed() {
    let bus = EventBus::builder().buffer_size(32).build().await.expect("valid config");

    let sub = bus.subscribe::<PropEvent, _, _>(|_event: &PropEvent| Ok(())).await.expect("subscribe");

    let id = sub.id();
    assert!(sub.unsubscribe().await.expect("unsubscribe"));
    assert!(!bus.unsubscribe(id).await.expect("second unsubscribe"));

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn publish_after_shutdown_is_stopped() {
    let bus = EventBus::builder().buffer_size(32).build().await.expect("valid config");
    bus.shutdown().await.expect("shutdown");

    let err = bus.publish(PropEvent { _value: 1 }).await.expect_err("publish after shutdown");
    assert_eq!(err, EventBusError::Stopped);
}
