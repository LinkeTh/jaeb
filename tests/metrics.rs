#![cfg(feature = "metrics")]

use std::any::type_name;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{AsyncSubscriptionPolicy, DeadLetter, EventBus, HandlerResult, SyncEventHandler};
use metrics_util::debugging::{DebugValue, DebuggingRecorder};

type SnapshotEntries = Vec<(
    metrics_util::CompositeKey,
    Option<metrics::Unit>,
    Option<metrics::SharedString>,
    DebugValue,
)>;

#[derive(Clone, Debug)]
struct MetricEvent;

#[derive(Clone, Debug)]
struct RetryMetricEvent;

#[derive(Clone, Debug)]
struct DeadLetterMetricEvent;

struct DeadLetterCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<DeadLetter> for DeadLetterCounter {
    fn handle(&self, _event: &DeadLetter, _bus: &EventBus) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn labels_match(key: &metrics::Key, required: &[(&str, &str)]) -> bool {
    required
        .iter()
        .all(|(required_key, required_value)| key.labels().any(|label| label.key() == *required_key && label.value() == *required_value))
}

fn counter_value(entries: &SnapshotEntries, metric_name: &str, required_labels: &[(&str, &str)]) -> u64 {
    entries
        .iter()
        .filter_map(|(key, _unit, _desc, value)| {
            if key.key().name() != metric_name || !labels_match(key.key(), required_labels) {
                return None;
            }

            match value {
                DebugValue::Counter(v) => Some(*v),
                _ => None,
            }
        })
        .sum()
}

fn histogram_samples(entries: &SnapshotEntries, metric_name: &str, required_labels: &[(&str, &str)]) -> usize {
    entries
        .iter()
        .filter_map(|(key, _unit, _desc, value)| {
            if key.key().name() != metric_name || !labels_match(key.key(), required_labels) {
                return None;
            }

            match value {
                DebugValue::Histogram(values) => Some(values.len()),
                _ => None,
            }
        })
        .sum()
}

#[tokio::test(flavor = "current_thread")]
async fn publish_increments_event_counter() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let _sub = bus
        .subscribe::<MetricEvent, _, _>(|_event: &MetricEvent, _bus: &EventBus| Ok(()))
        .await
        .expect("subscribe");

    for _ in 0..3 {
        bus.publish(MetricEvent).await.expect("publish");
    }
    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(&snapshot, "eventbus.publish", &[("event", type_name::<MetricEvent>())]);
    assert_eq!(value, 3);
}

#[tokio::test(flavor = "current_thread")]
async fn listener_dispatch_records_histogram_values() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let _sub = bus
        .subscribe::<MetricEvent, _, _>(|_event: &MetricEvent, _bus: &EventBus| Ok(()))
        .await
        .expect("subscribe");

    for _ in 0..4 {
        bus.publish(MetricEvent).await.expect("publish");
    }
    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let samples = histogram_samples(&snapshot, "eventbus.handler.duration", &[("event", type_name::<MetricEvent>())]);
    assert_eq!(samples, 4);
}

#[tokio::test(flavor = "current_thread")]
async fn retry_metrics_error_counter_tracks_terminal_failure() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let policy = AsyncSubscriptionPolicy::default().with_max_retries(2).with_dead_letter(false);

    let _sub = bus
        .subscribe_with_policy::<RetryMetricEvent, _, _>(
            |_event: RetryMetricEvent, _bus: EventBus| async move { Err::<(), _>("always fail".into()) },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(RetryMetricEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(&snapshot, "eventbus.handler.error", &[("event", type_name::<RetryMetricEvent>())]);
    assert_eq!(value, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn dead_letter_publication_is_counted() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let dead_letter_hits = Arc::new(AtomicUsize::new(0));

    let _dl = bus
        .subscribe_dead_letters(DeadLetterCounter {
            count: Arc::clone(&dead_letter_hits),
        })
        .await
        .expect("subscribe dead letters");

    let _sub = bus
        .subscribe::<DeadLetterMetricEvent, _, _>(|_event: &DeadLetterMetricEvent, _bus: &EventBus| Err::<(), _>("boom".into()))
        .await
        .expect("subscribe failing handler");

    bus.publish(DeadLetterMetricEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(dead_letter_hits.load(Ordering::SeqCst), 1);

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(&snapshot, "eventbus.publish", &[("event", type_name::<DeadLetter>())]);
    assert_eq!(value, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn dead_letter_counter_fires_on_creation() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let _sub = bus
        .subscribe::<DeadLetterMetricEvent, _, _>(|_event: &DeadLetterMetricEvent, _bus: &EventBus| Err::<(), _>("boom".into()))
        .await
        .expect("subscribe failing handler");

    bus.publish(DeadLetterMetricEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(&snapshot, "eventbus.dead_letter", &[("event", type_name::<DeadLetterMetricEvent>())]);
    assert_eq!(value, 1, "dead letter counter must fire exactly once");
}

#[tokio::test(flavor = "current_thread")]
async fn dead_letter_counter_does_not_fire_when_disabled() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let policy = AsyncSubscriptionPolicy::default().with_dead_letter(false);
    let _sub = bus
        .subscribe_with_policy::<DeadLetterMetricEvent, _, _>(
            |_event: DeadLetterMetricEvent, _bus: EventBus| async move { Err::<(), _>("boom".into()) },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(DeadLetterMetricEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(&snapshot, "eventbus.dead_letter", &[("event", type_name::<DeadLetterMetricEvent>())]);
    assert_eq!(value, 0, "dead letter counter must not fire when dead_letter=false");
}

#[tokio::test(flavor = "current_thread")]
async fn dead_letter_counter_includes_handler_label_when_named() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    struct NamedFailHandler;
    impl SyncEventHandler<DeadLetterMetricEvent> for NamedFailHandler {
        fn handle(&self, _: &DeadLetterMetricEvent, _bus: &EventBus) -> HandlerResult {
            Err("always fail".into())
        }
        fn name(&self) -> Option<&'static str> {
            Some("named-fail-handler")
        }
    }

    let bus = EventBus::builder().build().await.expect("valid config");
    let _sub = bus.subscribe::<DeadLetterMetricEvent, _, _>(NamedFailHandler).await.expect("subscribe");

    bus.publish(DeadLetterMetricEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(
        &snapshot,
        "eventbus.dead_letter",
        &[("event", type_name::<DeadLetterMetricEvent>()), ("handler", "named-fail-handler")],
    );
    assert_eq!(value, 1, "dead letter counter must include handler label when handler is named");
}

#[tokio::test(flavor = "current_thread")]
async fn metrics_snapshot_after_shutdown_contains_recorded_values() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let _sub = bus
        .subscribe::<MetricEvent, _, _>(|_event: &MetricEvent, _bus: &EventBus| Ok(()))
        .await
        .expect("subscribe");

    bus.publish(MetricEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(&snapshot, "eventbus.publish", &[("event", type_name::<MetricEvent>())]);
    assert_eq!(value, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_publish_metrics_coherent() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let bus = EventBus::builder().build().await.expect("valid config");
    let _sub = bus
        .subscribe::<MetricEvent, _, _>(|_event: &MetricEvent, _bus: &EventBus| Ok(()))
        .await
        .expect("subscribe");

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let bus_cloned = bus.clone();
        tasks.spawn(async move {
            for _ in 0..20 {
                bus_cloned.publish(MetricEvent).await.expect("publish");
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.expect("task should not panic");
    }

    bus.shutdown().await.expect("shutdown");

    let snapshot = snapshotter.snapshot().into_vec();
    let value = counter_value(&snapshot, "eventbus.publish", &[("event", type_name::<MetricEvent>())]);
    assert_eq!(value, 200);
}
