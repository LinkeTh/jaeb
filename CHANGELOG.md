# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.2] - 2026-04-09

### Fixed

- Async handler failures are now reaped promptly even when the bus is idle,
  using `tokio::select!` in the actor loop instead of polling only after
  processing a new message.
- Handler panics now flow through the retry/dead-letter pipeline instead of
  being silently logged and dropped.
- Type-erasure downcast failures now return an error instead of silently
  succeeding with `Ok(())`.
- Fixed redundant doc-link target in crate-level documentation.

### Changed

- `Subscription` is now `#[must_use]` to prevent accidental drops that leave
  listeners registered.
- `SubscriptionId` generation uses `checked_add` and panics on overflow instead
  of silently saturating at `u64::MAX`.
- Narrowed `tokio` dependency from `features = ["full"]` to only the features
  actually used (`rt`, `rt-multi-thread`, `sync`, `macros`, `time`).
- Replaced sleep-based test synchronization with deterministic `shutdown()`
  draining for more reliable CI.
- CI now tests the full workspace (`--workspace`) including `summer-jaeb` and
  `summer-jaeb-macros`.
- Improved `jaeb-demo` example: reduced 20s sleep to 1s + shutdown, added
  `PaymentFailedEvent` demonstrating the dead-letter pipeline end-to-end.

### Added

- Comprehensive doc comments on `FailurePolicy` fields, `DeadLetter` struct,
  `EventBus::unsubscribe`, and crate-level Tokio runtime requirement.

### Added (summer-jaeb-macros 0.1.2)

- `#[event_listener]` now validates that the return type is `HandlerResult`.
- Duplicate macro attributes (e.g. `retries` specified twice) now produce a
  compile error instead of silently using the last value.
- Split `lib.rs` into `attrs`, `validate`, and `codegen` modules.
- Collapsed nested `if` blocks to use Rust 2024 let-chains.

### Fixed (summer-jaeb 0.1.2)

- Removed redundant `#[must_use]` on internal `auto_listeners()` function
  (clippy `double_must_use`).
- Documented plugin startup panic behavior.

## [0.2.1]

### Fixed

- Documentation and metadata updates.

## [0.2.0]

### Changed

- `EventBus::shutdown()` now returns `Err(EventBusError::ShutdownTimeout)` when a
  configured `shutdown_timeout` deadline expires and in-flight async tasks are
  forcibly aborted. Previously, shutdown always returned `Ok(())` even when tasks
  were aborted.

## [0.1.0]

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
