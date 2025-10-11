use jaeb::{bootstrap_listeners, event_listener, EventBus};
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::MetricKindMask;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::thread;
use std::time::Duration;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Clone, Debug)]
struct OrderCheckOutEvent {
    order_id: i32,
}

#[derive(Clone, Debug)]
struct OrderCancelledEvent {
    order_id: i32,
}

async fn checkout(order_id: i32, bus: &EventBus) {
    // domain logic
    // order.checkout();

    // publish both ways
    bus.publish(OrderCheckOutEvent { order_id }).await;
    bus.publish(OrderCancelledEvent { order_id }).await;
}

#[event_listener]
fn on_cancel(e: &OrderCancelledEvent) {
    // do synchronous side effects
    info!("sync: order {} cancelled", e.order_id);
}

#[event_listener]
async fn on_checkout_async(e: OrderCheckOutEvent) {
    // do async work, e.g. send email, call other service
    // wait for 5 secs
    // tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    info!("async: order {} checked out", e.order_id);
}

#[event_listener]
fn on_checkout(e: &OrderCheckOutEvent) {
    // do synchronous side effects
    // wait for 2 secs
    // thread::sleep(Duration::from_secs(2));
    info!("sync: order {} checked out", e.order_id);
}

#[tokio::main]
async fn main() {
    // Install a global tracing subscriber with env-based filtering
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,metrics=trace,jaeb=trace,jaeb::event_bus=trace"));

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                // .json()
                .with_target(true)
                .with_thread_ids(true)
                .with_level(true),
        )
        .with(filter)
        .init();

    init_prometheus(&3000);

    let bus = EventBus::new(64);
    // Automatically registers all #[event_listener] functions in the binary
    bootstrap_listeners!(&bus);

    checkout(42, &bus).await;
    tokio::time::sleep(Duration::from_secs(20)).await;

    // loop {
    //     tokio::time::sleep(Duration::from_millis(2000)).await;
    //     checkout(42, &bus).await;
    // }
}

fn init_prometheus(port: &u16) {
    PrometheusBuilder::new()
        .idle_timeout(MetricKindMask::COUNTER | MetricKindMask::HISTOGRAM, Some(Duration::from_secs(10)))
        .with_http_listener(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port.to_owned()))
        .install()
        .expect("failed to install Prometheus recorder");
}
