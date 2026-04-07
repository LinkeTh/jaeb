use jaeb::{EventBus, EventHandler, SyncEventHandler, async_trait};
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::MetricKindMask;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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

// ── Handlers ────────────────────────────────────────────────────────────

/// Handles order checkout -- in a real app this would hold PgPool, mailer, etc.
struct OnOrderCheckout {
    // pool: PgPool,
}

#[async_trait]
impl EventHandler<OrderCheckOutEvent> for OnOrderCheckout {
    async fn handle(&self, event: &OrderCheckOutEvent) {
        // async work: send email, update DB via self.pool, etc.
        info!("async: order {} checked out", event.order_id);
    }
}

/// Handles order cancellation synchronously.
struct OnOrderCancelled;

impl SyncEventHandler<OrderCancelledEvent> for OnOrderCancelled {
    fn handle(&self, event: &OrderCancelledEvent) {
        info!("sync: order {} cancelled", event.order_id);
    }
}

// ── App logic ───────────────────────────────────────────────────────────

async fn cancellation(order_id: i32, bus: &EventBus) {
    // domain logic
    // order.cancel();

    // publish
    bus.publish(OrderCancelledEvent { order_id }).await;
}
async fn checkout(order_id: i32, bus: &EventBus) {
    // domain logic
    // order.checkout();

    // publish
    bus.publish(OrderCheckOutEvent { order_id }).await;
}

async fn register_listeners(bus: &EventBus) {
    // In a real app, pass pool/config/etc. into handler structs here:
    //   bus.register(OnOrderCheckout { pool: pool.clone() }).await;
    bus.register(OnOrderCheckout { /* pool */ }).await;
    bus.register_sync(OnOrderCancelled).await;

    // Closures for simple one-off listeners:
    bus.subscribe_async(|e: OrderCheckOutEvent| async move {
        info!("closure: order {} checked out", e.order_id);
    })
    .await;
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

    let bus = EventBus::new(64);
    register_listeners(&bus).await;

    checkout(42, &bus).await;
    cancellation(42, &bus).await;
    tokio::time::sleep(Duration::from_secs(20)).await;
}

fn init_prometheus(port: &u16) {
    PrometheusBuilder::new()
        .idle_timeout(MetricKindMask::COUNTER | MetricKindMask::HISTOGRAM, Some(Duration::from_secs(10)))
        .with_http_listener(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port.to_owned()))
        .install()
        .expect("failed to install Prometheus recorder");
}
