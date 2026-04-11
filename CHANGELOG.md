# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.5] - 2026-04-11

### Changed

- Reduced async publish-path overhead in `AsyncTaskTracker` by disabling
  abort-handle map bookkeeping when `shutdown_timeout` is not configured,
  while preserving timeout-based shutdown abort semantics when it is configured.
- Reduced per-listener async dispatch capture size by introducing lightweight
  listener metadata for retry/dead-letter handling in the spawn path.
- Split snapshot listener storage into typed `AsyncListenerEntry` and
  `SyncListenerEntry`, eliminating per-dispatch enum matching.
- Added fast-path in dispatch that skips middleware iteration when none
  are registered.
- Eliminated per-publish `DispatchContext` allocation overhead by switching
  from owned `Arc`/sender fields to borrowed references, removing 4 atomic
  operations (2 clone + 2 drop) per publish call.

### Fixed

- Fixed TOCTOU race in `AsyncTaskTracker::finish_task` — replaced
  separate load + compare with `fetch_sub` return-value check.

## [0.3.4] - 2026-04-11

### Added

- New standalone proc-macro crate `jaeb-macros` with:
    - `#[handler]` for generating JAEB handler structs from free functions
    - `register_handlers!(bus, ...)` for batch async registration with `?`-based error propagation
    - `register_handlers!(bus)` for `inventory`-based auto-discovery and registration
- New optional `macros` feature on `jaeb` that re-exports `handler` and
  `register_handlers`, and enables internal `inventory` support required for
  auto-discovery.
- New `examples/macro-handlers` demonstrating standalone macro usage without
  summer-rs.
- New `examples/macro-handlers-auto` showing zero-argument macro registration.

### Changed

- Optimized async dispatch hot path by removing an internal nested
  `tokio::spawn` during listener execution; async handler futures are now
  executed directly in the tracked task with panic capture.
- Reduced tracker overhead by switching internal async-task handle storage from
  `tokio::sync::Mutex` to `std::sync::Mutex` and making task tracking updates
  synchronous.
- Consolidated panic-message extraction for sync and async handler paths.
- Updated `BENCHMARK.md` with new cross-library measurements after the
  dispatch-path optimization pass.

### Performance

- `comparison_async_single_listener` (`jaeb_publish`): ~`1.96 us` -> ~`1.39 us`
  (~29% faster)
- `comparison_async_fanout_10` (`jaeb_publish`): ~`19.16 us` -> ~`10.64 us`
  (~45% faster)
- `comparison_contention_4_publishers` (`jaeb_publish`): ~`1.83 us` -> ~`0.45 us`
  (~75% faster)

## [0.3.3] - 2026-04-11

### Added

- Listener priority support via `priority` on subscription policies, with
  higher-priority listeners dispatched first and stable registration order for
  equal priority.
- New policy types:
    - `SubscriptionPolicy { priority, max_retries, retry_strategy, dead_letter }`
    - `SyncSubscriptionPolicy { priority, dead_letter }`
- New compile-time compatibility trait name: `IntoSubscriptionPolicy<M>`.
- Priority integration tests in `tests/priority.rs`.
- Cross-library criterion benchmark harness in `benches/comparison.rs` for
  `jaeb`, `eventbuzz`, and `evno` (where stable).
- `BENCHMARK.md` with benchmark methodology, latest measurements, and caveats.
- New `examples/axum-integration` showcasing REST endpoints that publish domain
  events and consume dead letters.

### Changed

- Renamed public policy API from failure-centric names to subscription-centric
  names throughout docs and examples.
- `EventBusBuilder::default_failure_policy(...)` renamed to
  `default_subscription_policy(...)`.
- `TestBusBuilder::default_failure_policy(...)` renamed to
  `default_subscription_policy(...)`.
- `#[event_listener]` macro now supports a `priority = <int>` attribute and
  generates policy chains using `SubscriptionPolicy` / `SyncSubscriptionPolicy`.
- README was expanded and updated with architecture, policy model, benchmark,
  and example guidance.

### Deprecated

- `FailurePolicy` (use `SubscriptionPolicy`).
- `NoRetryPolicy` (use `SyncSubscriptionPolicy`).
- `IntoFailurePolicy` (use `IntoSubscriptionPolicy`).
- `EventBusBuilder::default_failure_policy(...)` (use
  `default_subscription_policy(...)`).
- `TestBusBuilder::default_failure_policy(...)` (use
  `default_subscription_policy(...)`).

### Notes

- `evno` contention benchmark is intentionally omitted from the active
  criterion group due to reproducible instability (high CPU/non-terminating
  behavior) in this environment; details are documented in `BENCHMARK.md`.

## [0.3.2] - 2026-04-10

### Added

- Added missing public API comments/docs.

### Changed (summer-jaeb 0.2.1)

- Minor version update

## [0.3.1] - 2026-04-10

### Added

- Multiple examples in the `examples/` directory

### Changed (summer-jaeb 0.2.0)

- Minor version update

## [0.3.0] - 2026-04-10

### Changed

- Renamed `EventBusError::ActorStopped` to `EventBusError::Stopped` to use
  architecture-neutral terminology.
- **Major dispatch architecture redesign for latency.** Replaced the
  actor->worker dispatch roundtrip with direct publish-path dispatch over an
  `ArcSwap` snapshot registry (`src/registry.rs`).
- **Dual-lane per-type dispatch.** Sync handlers remain serialized (FIFO), while
  async handlers are spawned on a separate lane so same-type async work is no
  longer stalled by slow sync handlers.
- **Backpressure semantics updated.** `try_publish` now returns
  `EventBusError::ChannelFull` when immediate dispatch capacity/permits are not
  available, rather than actor-mailbox fullness.
- **Control loop simplified.** Control flow now focuses on failure/dead-letter
  notifications and shutdown orchestration.
- Removed internal `actor.rs` + `worker.rs` dispatch machinery and replaced
  with snapshot-driven dispatch internals.

### Removed

- Internal `actor.rs` and `worker.rs` dispatch model in favor of snapshot
  dispatch internals.

## [0.2.4] - 2026-04-09

### Changed

- Refactored internal dispatch from per-handler `dyn Any` downcasts to a typed
  listener registry keyed by event `TypeId`, preserving the single-actor
  orchestration model and existing public API behavior.

### Added

- Closure handler support for `EventBus::subscribe*` APIs:
    - sync closures: `Fn(&E) -> HandlerResult`
    - async closures: `Fn(E) -> impl Future<Output = HandlerResult>`
- Typed per-event middleware:
    - `TypedMiddleware<E>` (async)
    - `TypedSyncMiddleware<E>` (sync)
    - new APIs `EventBus::add_typed_middleware` and
      `EventBus::add_typed_sync_middleware`
- New integration coverage for closure handlers and typed middleware behavior.

## [summer-jaeb 0.1.4] - 2026-04-09

### Added

- **Plugin dependency support for `SummerJaeb`.** `SummerJaeb` is now a builder
  struct (was a unit struct). Use `SummerJaeb::new()` or `SummerJaeb::default()`
  where you previously wrote `SummerJaeb`. Declare plugin build-order
  dependencies via `.with_dependency("PluginName")` or
  `.with_dependencies(["A", "B"])`, which overrides `Plugin::dependencies()` so
  summer-rs builds the named plugins first. This eliminates non-deterministic
  startup panics when `#[event_listener]` functions inject components from
  plugins that may not yet have been built.

### Breaking Changes

- `SummerJaeb` is no longer a unit struct. All uses of the bare `SummerJaeb`
  expression must be replaced with `SummerJaeb::new()` (or
  `SummerJaeb::default()`).

## [0.2.3] - 2026-04-09

### Breaking Changes

- **`DeadLetter` struct has new fields.** `event: Arc<dyn Any + Send + Sync>`
  holds the original event payload, `failed_at: SystemTime` records when the
  failure occurred, and `listener_name: Option<&'static str>` identifies the
  handler that failed. Code that constructs or pattern-matches `DeadLetter`
  must be updated.
- **`EventBus::publish()` now returns middleware rejections.** The return type
  is unchanged (`Result<(), EventBusError>`) but a new
  `EventBusError::MiddlewareRejected(String)` variant may be returned when a
  middleware rejects the event.
- **`subscribe_with_policy` now requires `IntoFailurePolicy<M>` instead of
  `FailurePolicy`.** Passing a `FailurePolicy` (with retries) to a sync handler
  is now a compile error. Use `NoRetryPolicy` for sync handlers.
- **Removed `EventBusError::SyncRetryNotSupported`.** This runtime check is no
  longer needed because the type system prevents the invalid combination at
  compile time.
- **Removed `retry_delay_ms`. Use `retry_strategy = "fixed", retry_base_ms = N` instead.

### Added

- **Richer dead-letter context.** `DeadLetter` now carries the original event
  payload (`event`), a failure timestamp (`failed_at`), and the listener name
  (`listener_name`). Consumers can downcast `event` back to the original type.
- **Named subscriptions.** `EventHandler` and `SyncEventHandler` gain an
  optional `fn name(&self) -> Option<&'static str>` default method. The name
  is threaded through to `DeadLetter`, tracing spans, and introspection.
- **Introspection API.** `EventBus::stats()` returns a `BusStats` snapshot
  with total subscription count, per-event-type listener info (including
  names), registered event types, in-flight async task count, queue capacity,
  and shutdown status. New types: `BusStats`, `ListenerInfo`.
- **Once-off subscriptions.** `EventBus::subscribe_once()` and
  `subscribe_once_with_policy()` register handlers that fire exactly once and
  are then auto-removed. Retries are forced to zero. The `Subscription`
  handle's `unsubscribe()` returns `false` after auto-removal.
- **Middleware / interceptors.** `EventBus::add_middleware()` and
  `add_sync_middleware()` register cross-cutting, untyped middleware that runs
  before handler dispatch. Middleware returns `MiddlewareDecision::Continue` or
  `Reject(reason)`. On rejection, `publish()` returns
  `EventBusError::MiddlewareRejected`. Middlewares execute in FIFO order and
  can be removed via the returned `Subscription`. New traits: `Middleware`,
  `SyncMiddleware`. New type: `MiddlewareDecision`.
- **Test utilities.** Feature-gated `test-utils` module with `TestBus` — a
  wrapper around `EventBus` with per-type event capture buffers. Methods:
  `capture::<E>()`, `published::<E>()`, `assert_count::<E>(n)`,
  `assert_empty::<E>()`. Also includes `TestBusBuilder`.
- **`IntoFailurePolicy<M>` sealed trait.** Leverages existing `AsyncMode` /
  `SyncMode` marker types to enforce at compile time that sync handlers cannot
  be given retry policies.
- **`NoRetryPolicy` supports both sync and async handlers.** `NoRetryPolicy`
  implements `IntoFailurePolicy<AsyncMode>` as well, so it can be used with
  async handlers when retries are not desired.

### Changed (summer-jaeb-macros 0.1.3)

- `#[event_listener]` on sync handlers now generates `::jaeb::NoRetryPolicy`
  chains instead of `::jaeb::FailurePolicy`.
- Removed `retry_delay_ms` from all error messages and documentation.

### Added (summer-jaeb-macros 0.1.3)

- **Named subscriptions in `#[event_listener]`.** Macro-generated handlers now
  override `fn name()` automatically. By default the function name is used;
  `name = "custom"` overrides it; `name = ""` opts out (returns `None`).
- Attribute-parsing unit tests (13 tests covering name, retry, strategy, and
  error cases).

### Added (summer-jaeb 0.1.3)

- trybuild compile-fail test suite (12 tests covering all macro error paths).
- Config unit tests for deserialization and defaults.
- `serde_json` dev-dependency for config tests.

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
