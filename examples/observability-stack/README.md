# observability-stack

A full-stack observability example for [jaeb](../../README.md) using Docker Compose.

Demonstrates:

- **Metrics** — Prometheus pull endpoint + pre-built Grafana dashboard
- **Distributed tracing** — spans exported via OTLP gRPC to Grafana Tempo
- **Structured logs** — pushed directly to Grafana Loki via `tracing-loki` (no Promtail)
- **Automatic span propagation** — jaeb's `trace` feature captures `Span::current()` at publish time and instruments every async handler future; no
  event payload changes required

## Services

| Service    | URL                             | Notes                      |
|------------|---------------------------------|----------------------------|
| Grafana    | <http://localhost:3001>         | admin / admin              |
| Prometheus | <http://localhost:9090>         |                            |
| Loki       | <http://localhost:3100>         | HTTP API                   |
| Tempo      | <http://localhost:3200>         | HTTP query API             |
| App        | <http://localhost:3000/metrics> | Prometheus scrape endpoint |

## Quick start

**Prerequisites:** Docker with the Compose plugin (v2.x) and ~2 GB of free disk for images.

```sh
cd examples/observability-stack/docker
docker compose up --build
```

The first run pulls images and compiles the Rust binary inside the builder stage — expect 3–5 minutes. Subsequent runs are fast.

Once all services are healthy, open the pre-provisioned dashboard:

**<http://localhost:3001/d/jaeb-overview>**

No login is required (anonymous viewer access is enabled). To log in as admin use `admin` / `admin`.

## Dashboard panels

The `jaeb Overview` dashboard has four rows:

| Row                 | Panels                                                                          |
|---------------------|---------------------------------------------------------------------------------|
| Publish Throughput  | Publish rate, cumulative publish volume                                         |
| Handler Performance | Handler duration heatmap, p50/p95/p99 latency per handler                       |
| Errors & Retries    | Handler error rate, retry count, dead-letter counter                            |
| Tracing             | Link to Tempo — click any trace ID in Loki to jump directly to a span waterfall |

Trace-to-metrics correlation is handled by Tempo's `metrics_generator` and Grafana datasource cross-linking. To explore a trace:

1. Open **Explore** → select the **Loki** datasource.
2. Query `{app="observability-stack"}` and find a log line with a `trace_id` field.
3. Click the **Tempo** link next to the trace ID to open the span waterfall.

## Simulation

The app publishes a continuous mixed stream of four event types:

| Event                | Rate         | Handlers                                                      |
|----------------------|--------------|---------------------------------------------------------------|
| `OrderCreated`       | every tick   | `order-persist` (fast async), `order-email` (slow async)      |
| `PaymentProcessed`   | 80% of ticks | `payment-ledger` (flaky, retries+DLQ), `payment-audit` (sync) |
| `ShipmentDispatched` | 60% of ticks | `warehouse-notify` (medium async)                             |
| `FraudCheckFailed`   | 10% of ticks | `fraud-alert` (always fails → dead-letter)                    |

Dead letters are received by `DeadLetterSink`, which increments the `jaeb.dead_letter.total` counter visible in the dashboard.

## Configuration

All options are set via environment variables. Defaults are suitable for the Docker Compose network:

| Variable          | Default                 | Description                                      |
|-------------------|-------------------------|--------------------------------------------------|
| `LOKI_URL`        | `http://localhost:3100` | Loki push endpoint                               |
| `OTLP_ENDPOINT`   | `http://localhost:4317` | Tempo OTLP gRPC receiver                         |
| `PROMETHEUS_PORT` | `3000`                  | Port for the `/metrics` scrape endpoint          |
| `TICK_MS`         | `500`                   | Base tick interval in milliseconds (±50% jitter) |
| `RUST_LOG`        | `info,jaeb=debug`       | `tracing` filter directive                       |

## Running locally (without Docker)

You can run the app binary directly against local instances of the backends:

```sh
# from the workspace root
cargo run -p observability-stack
```

You will need Loki, Tempo, and Prometheus reachable at their default localhost ports,
or override `LOKI_URL` / `OTLP_ENDPOINT` / `PROMETHEUS_PORT` as needed.

## Stopping

```sh
docker compose down          # stop containers, keep volumes
docker compose down -v       # stop and remove all volumes (fresh state)
```
