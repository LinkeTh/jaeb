# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- **Sync handlers no longer support retries.** Previously, sync handler retries
  with `retry_delay` would either block the actor loop (pre-0.2.2) or violate
  the "sync listeners complete before publish returns" contract (by spawning
  delayed retries into background tasks). Sync handlers now execute exactly once;
  on failure, a dead letter is emitted when enabled. Subscribing a sync handler
  with `max_retries > 0` returns `EventBusError::SyncRetryNotSupported`. This
  aligns with the ecosystem consensus that in-process sync dispatch is one-shot.
- **Actor `JoinHandle` is now stored and joined on shutdown.** The actor task
  handle was previously dropped immediately after spawning; it is now retained
  and awaited during `shutdown()` to observe panics.
- **Shutdown is now idempotent.** Calling `shutdown()` multiple times returns
  `Ok(())` on subsequent calls instead of erroring.

### Added

- Comprehensive doc comments on `FailurePolicy` fields, `DeadLetter` struct,
  `EventBus::unsubscribe`, and crate-level Tokio runtime requirement.
- `SubscriptionGuard` -- RAII wrapper that auto-unsubscribes on drop. Created
  via `Subscription::into_guard()`. Call `guard.disarm()` to prevent auto-
  unsubscribe.
- `EventBus::is_healthy()` -- returns whether the internal actor task is still
  running.
- `EventBusError::SyncRetryNotSupported` variant -- returned when subscribing a
  sync handler with `max_retries > 0`.
- `ConfigError` enum with `ZeroBufferSize` and `ZeroConcurrency` variants.
- `EventBusError::InvalidConfig(ConfigError)` variant for structured config
  validation errors.
- Async handler panic safety test.
- Concurrent publish/unsubscribe race test.
- `SubscriptionGuard` test suite (drop, disarm, post-shutdown safety).
- Backpressure test for `publish()` succeeding after channel drains.
- Mixed sync/async and contention benchmarks.

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
- **`EventBus::new()` is now fallible**, returning `Result<EventBus, EventBusError>`
  instead of `EventBus`. Use `.expect("valid config")` or `?` at call sites.
- Comprehensive doc comments added to `actor.rs` and `handler.rs` documenting
  allocation costs per publish, O(n) listener removal, and the `Clone`
  requirement on async event types.
- Actor loop now eagerly reaps completed async tasks before processing each
  message, preventing dead-letter starvation under sustained publish load.

### Changed (summer-jaeb-macros 0.1.2)

- `retry_delay_ms` without `retries` now produces a compile error instead of
  being silently ignored.
- `retries` and `retry_delay_ms` on sync (non-async) listener functions now
  produce a compile error — retries are only supported for async handlers.

### Changed (summer-jaeb 0.1.2)

- Config loading failure now falls back to defaults with a warning log instead
  of panicking.
- Plugin docs updated to note that automatic shutdown is not supported (summer's
  `Plugin` trait has no teardown hook).

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
