//! Telemetry initialization: Prometheus metrics, tracing-loki, tracing-opentelemetry.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::MetricKindMask;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt};
use url::Url;

/// Handles returned by [`init_tracing`] that must be kept alive until shutdown.
pub struct TelemetryHandles {
    pub loki_task: tokio::task::JoinHandle<()>,
    pub tracer_provider: SdkTracerProvider,
}

/// Install the global Prometheus metrics recorder with an HTTP scrape endpoint.
pub fn init_prometheus(port: u16) {
    PrometheusBuilder::new()
        .idle_timeout(MetricKindMask::COUNTER | MetricKindMask::HISTOGRAM, Some(Duration::from_secs(60)))
        .with_http_listener(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port))
        .install()
        .expect("Prometheus recorder install failed");
}

/// Initialize the tracing subscriber with three layers:
///
/// 1. `fmt::layer()` -- human-readable stdout output
/// 2. `tracing_loki::layer()` -- structured logs pushed directly to Loki
/// 3. `OpenTelemetryLayer` -- spans exported to Tempo via OTLP gRPC
pub async fn init_tracing(loki_url: &str, otlp_endpoint: &str) -> TelemetryHandles {
    // ── Loki layer ─────────────────────────────────────────────────────────
    let (loki_layer, loki_task) = tracing_loki::builder()
        .label("app", "observability-stack")
        .expect("valid label")
        .label("env", "docker")
        .expect("valid label")
        .extra_field("pid", format!("{}", std::process::id()))
        .expect("valid field")
        .build_url(Url::parse(loki_url).expect("invalid Loki URL"))
        .expect("Loki layer init failed");

    let loki_task = tokio::spawn(loki_task);

    // ── OTLP / Tempo layer (new 0.31 API) ──────────────────────────────────
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(otlp_endpoint)
        .build()
        .expect("OTLP exporter build failed");

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(Resource::builder().with_service_name("observability-stack").build())
        .with_batch_exporter(exporter)
        .build();

    let tracer = tracer_provider.tracer("observability-stack");
    let otel_layer = OpenTelemetryLayer::new(tracer);

    // ── Filter ─────────────────────────────────────────────────────────────
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,jaeb=debug"));

    // ── Compose and install ────────────────────────────────────────────────
    Registry::default()
        .with(filter)
        .with(fmt::layer().with_target(true).with_thread_ids(true).with_level(true))
        .with(loki_layer)
        .with(otel_layer)
        .init();

    TelemetryHandles { loki_task, tracer_provider }
}

/// Flush remaining telemetry data and shut down background tasks.
pub fn shutdown_telemetry(handles: TelemetryHandles) {
    if let Err(e) = handles.tracer_provider.shutdown() {
        tracing::warn!(error = %e, "tracer provider shutdown error");
    }
    handles.loki_task.abort();
}
