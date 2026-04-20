//! Telemetry initialization: Prometheus metrics, OpenTelemetry logs/traces, tracing-opentelemetry.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::MetricKindMask;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::BatchLogProcessor;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt};

/// Handles returned by [`init_tracing`] that must be kept alive until shutdown.
pub struct TelemetryHandles {
    pub logger_provider: SdkLoggerProvider,
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
/// 2. OpenTelemetry tracing bridge -- structured logs exported to the collector
/// 3. `OpenTelemetryLayer` -- spans exported to the collector via OTLP gRPC
pub async fn init_tracing(otlp_endpoint: &str) -> TelemetryHandles {
    // ── OpenTelemetry logs ────────────────────────────────────────────────
    let log_exporter = LogExporter::builder()
        .with_tonic()
        .with_endpoint(otlp_endpoint)
        .build()
        .expect("OTLP log exporter build failed");

    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(Resource::builder().with_service_name("observability-stack").build())
        .with_log_processor(BatchLogProcessor::builder(log_exporter).build())
        .build();

    let otel_log_layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider)
        .with_filter(tracing_subscriber::filter::LevelFilter::INFO);

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
        .with(otel_log_layer)
        .with(otel_layer)
        .init();

    TelemetryHandles {
        logger_provider,
        tracer_provider,
    }
}

/// Flush remaining telemetry data and shut down background tasks.
pub fn shutdown_telemetry(handles: TelemetryHandles) {
    if let Err(e) = handles.logger_provider.shutdown() {
        tracing::warn!(error = %e, "logger provider shutdown error");
    }
    if let Err(e) = handles.tracer_provider.shutdown() {
        tracing::warn!(error = %e, "tracer provider shutdown error");
    }
}
