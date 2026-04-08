# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - Unreleased

### Changed

- `EventBus::shutdown()` now returns `Err(EventBusError::ShutdownTimeout)` when a
  configured `shutdown_timeout` deadline expires and in-flight async tasks are
  forcibly aborted. Previously, shutdown always returned `Ok(())` even when tasks
  were aborted.

### Added

- Actor-based in-process event bus for Tokio applications.
- Typed event routing via `TypeId` with the blanket `Event` trait.
- Synchronous dispatch (`SyncEventHandler`) -- handlers run inline, publish
  awaits completion.
- Asynchronous dispatch (`EventHandler`) -- handlers run concurrently via
  `JoinSet`, publish returns immediately.
- `FailurePolicy` with configurable retries, retry delay, and dead-letter
  toggle.
- Dead-letter support with recursion guard preventing infinite loops.
- `Subscription` handles for unsubscribing by value or by `SubscriptionId`.
- `try_publish` for non-blocking, backpressure-aware publishing.
- Graceful `shutdown()` that drains queued messages and waits for in-flight
  async handlers.
- `EventBus::builder()` with `EventBusBuilder` for fine-grained configuration:
  - `buffer_size` -- internal channel capacity.
  - `handler_timeout` -- per-invocation timeout; exceeded handlers are treated
    as failures eligible for retry / dead-letter.
  - `max_concurrent_async` -- semaphore-bounded async task concurrency.
  - `default_failure_policy` -- applied to subscriptions that don't specify one.
  - `shutdown_timeout` -- deadline after which remaining async tasks are aborted.
- Optional `metrics` feature gate for Prometheus-compatible counters and
  histograms.
- Tracing instrumentation throughout the actor lifecycle.
- `ShutdownTimeout` error variant in `EventBusError`.
- CI/CD pipeline (GitHub Actions) with fmt, clippy, test matrix, doc build,
  and security audit.
- Criterion benchmarks for publish throughput, fan-out latency, and
  `try_publish`.
- Doc-tested examples on all public `EventBus` and `EventBusBuilder` methods.
- Crates.io metadata (`keywords`, `categories`, `readme`, `rust-version`).
