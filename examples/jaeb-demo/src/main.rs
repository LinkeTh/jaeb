// SPDX-License-Identifier: MIT
use std::sync::Arc;

use jaeb::{DeadLetter, EventBus, EventHandler, FailurePolicy, HandlerResult, RetryStrategy, SyncEventHandler};
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::MetricKindMask;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct OrderCheckOutEvent {
    order_id: i32,
}

#[derive(Clone, Debug)]
struct OrderCancelledEvent {
    order_id: i32,
}

#[derive(Clone, Debug)]
struct PaymentFailedEvent {
    order_id: i32,
}

// ── Handlers ────────────────────────────────────────────────────────────
struct DbPool;

impl DbPool {
    fn persist(&self, order_id: i32) {
        info!(order_id, "DbPool: persisted order to database");
    }
}
/// Handles order checkout -- in a real app this would hold PgPool, mailer, etc.
struct OnOrderCheckout {
    pool: DbPool,
    attempts: Arc<AtomicUsize>,
}

impl EventHandler<OrderCheckOutEvent> for OnOrderCheckout {
    async fn handle(&self, event: &OrderCheckOutEvent) -> HandlerResult {
        // async work: send email, update DB via self.pool, etc.
        self.pool.persist(event.order_id);
        // Simulate transient failure on first attempt.
        let previous = self.attempts.fetch_add(1, Ordering::SeqCst);
        if previous == 0 {
            return Err("transient checkout failure".into());
        }

        info!("async: order {} checked out", event.order_id);
        Ok(())
    }
}

/// Handles order cancellation synchronously.
struct OnOrderCancelled;

impl SyncEventHandler<OrderCancelledEvent> for OnOrderCancelled {
    fn handle(&self, event: &OrderCancelledEvent) -> HandlerResult {
        info!("sync: order {} cancelled", event.order_id);
        Ok(())
    }
}

/// Logs order checkouts (simple listener).
struct CheckoutLogger;

impl EventHandler<OrderCheckOutEvent> for CheckoutLogger {
    async fn handle(&self, event: &OrderCheckOutEvent) -> HandlerResult {
        info!("logger: order {} checked out", event.order_id);
        Ok(())
    }
}

/// Always-failing handler to demonstrate the dead-letter pipeline.
/// After exhausting retries the event is routed to the dead-letter listener.
struct OnPaymentFailed;

impl EventHandler<PaymentFailedEvent> for OnPaymentFailed {
    async fn handle(&self, event: &PaymentFailedEvent) -> HandlerResult {
        Err(format!("payment gateway unavailable for order {}", event.order_id).into())
    }
}

/// Logs dead letters.
struct DeadLetterLogger;

impl SyncEventHandler<DeadLetter> for DeadLetterLogger {
    fn handle(&self, dl: &DeadLetter) -> HandlerResult {
        info!(
            "dead-letter: event={} listener={} attempts={} error={}",
            dl.event_name, dl.subscription_id, dl.attempts, dl.error
        );
        Ok(())
    }
}

// ── App logic ───────────────────────────────────────────────────────────

async fn cancellation(order_id: i32, bus: &EventBus) {
    // domain logic
    // order.cancel();

    // publish
    bus.publish(OrderCancelledEvent { order_id })
        .await
        .expect("failed to publish cancellation event");
}
async fn checkout(order_id: i32, bus: &EventBus) {
    // domain logic
    // order.checkout();

    // publish
    bus.publish(OrderCheckOutEvent { order_id })
        .await
        .expect("failed to publish checkout event");
}

async fn subscribe_listeners(bus: &EventBus) {
    let attempts = Arc::new(AtomicUsize::new(0));

    let pool = DbPool;
    // In a real app, pass pool/config/etc. into handler structs here:
    let retry_policy = FailurePolicy::default()
        .with_max_retries(1)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(100)));

    let _ = bus
        .subscribe_with_policy(OnOrderCheckout { pool, attempts }, retry_policy)
        .await
        .expect("failed to subscribe async handler");

    let _ = bus.subscribe(OnOrderCancelled).await.expect("failed to subscribe sync handler");

    // Struct-based listener for simple one-off logging:
    let _ = bus.subscribe(CheckoutLogger).await.expect("failed to subscribe checkout logger");

    // Always-failing handler with dead-letter enabled to demonstrate the dead-letter pipeline.
    let dl_policy = FailurePolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(50)))
        .with_dead_letter(true);
    let _ = bus
        .subscribe_with_policy(OnPaymentFailed, dl_policy)
        .await
        .expect("failed to subscribe payment-failed handler");

    let _ = bus
        .subscribe_dead_letters(DeadLetterLogger)
        .await
        .expect("failed to subscribe dead-letter listener");
}

#[tokio::main]
async fn main() {
    // Install a global tracing subscriber with env-based filtering
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,metrics=trace,jaeb=trace,jaeb::event_bus=trace"));

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true).with_thread_ids(true).with_level(true))
        .with(filter)
        .init();

    init_prometheus(&3000);

    let bus = EventBus::new(64).expect("valid config");
    subscribe_listeners(&bus).await;

    checkout(42, &bus).await;
    cancellation(42, &bus).await;

    // Publish an event that will always fail, demonstrating the dead-letter path.
    bus.publish(PaymentFailedEvent { order_id: 99 })
        .await
        .expect("failed to publish payment-failed event");

    // Give async handlers and retries time to complete, then shut down cleanly.
    tokio::time::sleep(Duration::from_secs(1)).await;
    bus.shutdown().await.expect("failed to shutdown event bus");
}

fn init_prometheus(port: &u16) {
    PrometheusBuilder::new()
        .idle_timeout(MetricKindMask::COUNTER | MetricKindMask::HISTOGRAM, Some(Duration::from_secs(10)))
        .with_http_listener(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port.to_owned()))
        .install()
        .expect("failed to install Prometheus recorder");
}
