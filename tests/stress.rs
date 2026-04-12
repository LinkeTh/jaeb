use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, EventBusError, EventHandler, HandlerResult};

#[derive(Clone, Debug)]
struct StressEvent;

struct SlowAsyncCounter {
    count: Arc<AtomicUsize>,
    delay: Duration,
}

impl EventHandler<StressEvent> for SlowAsyncCounter {
    async fn handle(&self, _event: &StressEvent) -> HandlerResult {
        tokio::time::sleep(self.delay).await;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn hundred_concurrent_publishers() {
    let bus = EventBus::new(1024).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let count_for_handler = Arc::clone(&count);
    let _sub = bus
        .subscribe::<StressEvent, _, _>(move |_event: &StressEvent| {
            count_for_handler.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe");

    let mut publishers = tokio::task::JoinSet::new();
    for _ in 0..100 {
        let bus_cloned = bus.clone();
        publishers.spawn(async move {
            for _ in 0..1000 {
                bus_cloned.publish(StressEvent).await.expect("publish");
            }
        });
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(result) = publishers.join_next().await {
            result.expect("publisher task should not panic");
        }
    })
    .await
    .expect("publishers timed out");

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 100_000);
}

#[tokio::test]
async fn five_hundred_listeners_single_type() {
    let bus = EventBus::new(1024).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    for _ in 0..500 {
        let count_for_handler = Arc::clone(&count);
        let _sub = bus
            .subscribe::<StressEvent, _, _>(move |_event: &StressEvent| {
                count_for_handler.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .expect("subscribe");
    }

    bus.publish(StressEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 500);
}

#[tokio::test]
async fn hundred_thousand_events_throughput() {
    let bus = EventBus::new(2048).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let count_for_handler = Arc::clone(&count);
    let _sub = bus
        .subscribe::<StressEvent, _, _>(move |_event: &StressEvent| {
            count_for_handler.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe");

    tokio::time::timeout(Duration::from_secs(60), async {
        for _ in 0..100_000 {
            bus.publish(StressEvent).await.expect("publish");
        }
    })
    .await
    .expect("throughput loop timed out");

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 100_000);
}

#[tokio::test]
async fn mixed_sync_async_high_contention() {
    let bus = EventBus::new(2048).expect("valid config");
    let sync_hits = Arc::new(AtomicUsize::new(0));
    let async_hits = Arc::new(AtomicUsize::new(0));

    for _ in 0..50 {
        let hits = Arc::clone(&sync_hits);
        let _sub = bus
            .subscribe::<StressEvent, _, _>(move |_event: &StressEvent| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .expect("subscribe sync");
    }

    for _ in 0..50 {
        let hits = Arc::clone(&async_hits);
        let _sub = bus
            .subscribe::<StressEvent, _, _>(move |_event: StressEvent| {
                let hits = Arc::clone(&hits);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await
            .expect("subscribe async");
    }

    let mut publishers = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let bus_cloned = bus.clone();
        publishers.spawn(async move {
            for _ in 0..500 {
                bus_cloned.publish(StressEvent).await.expect("publish");
            }
        });
    }

    tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(result) = publishers.join_next().await {
            result.expect("publisher task should not panic");
        }
    })
    .await
    .expect("publisher contention timed out");

    bus.shutdown().await.expect("shutdown");

    let expected = 20 * 500 * 50;
    assert_eq!(sync_hits.load(Ordering::SeqCst), expected);
    assert_eq!(async_hits.load(Ordering::SeqCst), expected);
}

#[tokio::test]
async fn subscribe_unsubscribe_churn_during_publish() {
    let bus = EventBus::new(1024).expect("valid config");
    let delivered = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    let delivered_for_handler = Arc::clone(&delivered);
    let _baseline = bus
        .subscribe::<StressEvent, _, _>(move |_event: &StressEvent| {
            delivered_for_handler.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe baseline");

    let publisher = {
        let bus = bus.clone();
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            while !stop.load(Ordering::Acquire) {
                bus.publish(StressEvent).await.expect("publish");
                tokio::task::yield_now().await;
            }
        })
    };

    let churner = {
        let bus = bus.clone();
        tokio::spawn(async move {
            for _ in 0..1000 {
                let sub = bus
                    .subscribe::<StressEvent, _, _>(|_event: &StressEvent| Ok(()))
                    .await
                    .expect("subscribe churn");
                let _removed = sub.unsubscribe().await.expect("unsubscribe churn");
            }
        })
    };

    tokio::time::timeout(Duration::from_secs(60), churner)
        .await
        .expect("churn timed out")
        .expect("churn task panicked");

    stop.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(10), publisher)
        .await
        .expect("publisher drain timed out")
        .expect("publisher task panicked");

    assert!(bus.is_healthy().await, "bus should remain healthy");
    assert!(delivered.load(Ordering::SeqCst) > 0, "baseline listener should have received events");
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn backpressure_fairness_under_contention() {
    let bus = EventBus::new(1).expect("valid config");
    let handled = Arc::new(AtomicUsize::new(0));

    let _sub = bus
        .subscribe(SlowAsyncCounter {
            count: Arc::clone(&handled),
            delay: Duration::from_millis(2),
        })
        .await
        .expect("subscribe");

    let mut publishers = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let bus_cloned = bus.clone();
        publishers.spawn(async move {
            let mut sent = 0usize;
            for _ in 0..20 {
                bus_cloned.publish(StressEvent).await.expect("publish");
                sent += 1;
            }
            sent
        });
    }

    let mut per_publisher = Vec::new();
    tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(result) = publishers.join_next().await {
            per_publisher.push(result.expect("publisher task panicked"));
        }
    })
    .await
    .expect("publisher fairness run timed out");

    bus.shutdown().await.expect("shutdown");

    assert_eq!(per_publisher.len(), 10);
    assert!(per_publisher.iter().all(|count| *count == 20), "each publisher should make progress");
    assert_eq!(handled.load(Ordering::SeqCst), 200);
}

#[tokio::test]
async fn concurrent_stats_access_under_load() {
    let bus = EventBus::new(1024).expect("valid config");
    let handled = Arc::new(AtomicUsize::new(0));

    let handled_for_handler = Arc::clone(&handled);
    let _sub = bus
        .subscribe::<StressEvent, _, _>(move |_event: &StressEvent| {
            handled_for_handler.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("subscribe");

    let mut stats_tasks = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let bus_cloned = bus.clone();
        stats_tasks.spawn(async move {
            for _ in 0..200 {
                let stats = bus_cloned.stats().await.expect("stats");
                assert!(stats.publish_permits_available <= stats.queue_capacity);
            }
        });
    }

    for _ in 0..10_000 {
        bus.publish(StressEvent).await.expect("publish");
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(result) = stats_tasks.join_next().await {
            result.expect("stats task panicked");
        }
    })
    .await
    .expect("stats tasks timed out");

    bus.shutdown().await.expect("shutdown");
    assert_eq!(handled.load(Ordering::SeqCst), 10_000);
}

#[tokio::test]
async fn memory_no_listener_accumulation() {
    let bus = EventBus::new(256).expect("valid config");

    for _ in 0..10_000 {
        let sub = bus
            .subscribe::<StressEvent, _, _>(|_event: &StressEvent| Ok(()))
            .await
            .expect("subscribe");
        let removed = sub.unsubscribe().await.expect("unsubscribe");
        assert!(removed, "subscription should exist");
    }

    let stats = bus.stats().await.expect("stats");
    assert_eq!(stats.total_subscriptions, 0);
    assert!(stats.registered_event_types.is_empty());

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn shutdown_with_many_inflight_async() {
    let bus = EventBus::builder()
        .buffer_size(2048)
        .shutdown_timeout(Duration::from_millis(30))
        .build()
        .expect("valid config");

    let completed = Arc::new(AtomicUsize::new(0));
    for _ in 0..200 {
        let completed_for_handler = Arc::clone(&completed);
        let _sub = bus
            .subscribe::<StressEvent, _, _>(move |_event: StressEvent| {
                let completed_for_handler = Arc::clone(&completed_for_handler);
                async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    completed_for_handler.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await
            .expect("subscribe async");
    }

    bus.publish(StressEvent).await.expect("publish");
    let shutdown_result = bus.shutdown().await;
    assert_eq!(shutdown_result, Err(EventBusError::ShutdownTimeout));
    assert_eq!(completed.load(Ordering::SeqCst), 0);

    let post_shutdown = bus.publish(StressEvent).await.expect_err("publish after shutdown");
    assert_eq!(post_shutdown, EventBusError::Stopped);
}

#[tokio::test]
async fn subscription_id_uniqueness_under_contention() {
    let bus = EventBus::new(1024).expect("valid config");
    let ids = Arc::new(tokio::sync::Mutex::new(HashSet::<u64>::new()));

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..1000 {
        let bus_cloned = bus.clone();
        let ids_cloned = Arc::clone(&ids);
        tasks.spawn(async move {
            let sub = bus_cloned
                .subscribe::<StressEvent, _, _>(|_event: &StressEvent| Ok(()))
                .await
                .expect("subscribe");
            ids_cloned.lock().await.insert(sub.id().as_u64());
        });
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(result) = tasks.join_next().await {
            result.expect("task panicked");
        }
    })
    .await
    .expect("subscribe contention timed out");

    assert_eq!(ids.lock().await.len(), 1000);
    bus.shutdown().await.expect("shutdown");
}
