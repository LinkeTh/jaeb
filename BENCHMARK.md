# Benchmark Results

This document records criterion-based cross-library benchmarks for:

- `jaeb`
- `eventbuzz` (`asynchronous` feature)
- `evno`

The benchmark code lives in `benches/comparison.rs`.

## Environment

- Date: 2026-04-11
- CPU/OS: local Linux development machine
- Runtime: Tokio multi-thread runtime with 4 worker threads (from benchmark harness)
- Tooling: `cargo bench --bench comparison`

## Reproduce

```sh
cargo bench --bench comparison
```

## Scenarios

- `comparison_async_single_listener`: one async no-op listener
- `comparison_async_fanout_10`: ten async no-op listeners
- `comparison_async_fanout_25`: twenty-five async no-op listeners
- `comparison_async_fanout_50`: fifty async no-op listeners
- `comparison_mixed_fanout_multi_type`: mixed publish stream across two event types with fanout
- `comparison_contention_4_publishers`: four concurrent publishers, one async no-op listener

## Latest Measurements (median-ish criterion time window)

| Group                                | Case                      |    Time |
|--------------------------------------|---------------------------|--------:|
| `comparison_async_single_listener`   | `jaeb_publish`            | 1.19 us |
| `comparison_async_single_listener`   | `eventbuzz_publish_event` | 0.93 us |
| `comparison_async_single_listener`   | `evno_emit`               | 0.60 us |
| `comparison_async_fanout_10`         | `jaeb_publish`            | 8.93 us |
| `comparison_async_fanout_10`         | `eventbuzz_publish_event` | 6.76 us |
| `comparison_async_fanout_10`         | `evno_emit`               | 5.02 us |
| `comparison_contention_4_publishers` | `jaeb_publish`            | 0.33 us |
| `comparison_contention_4_publishers` | `eventbuzz_publish_event` | 0.25 us |

Notes:

- Criterion reports a confidence interval, not a single exact scalar. Values above are rounded from the reported central range.
- These numbers are for relative orientation only, not absolute SLA guarantees.
- Relative to the initial (pre-optimization) measurements documented in `CHANGELOG.md`, `jaeb` improved by roughly:
    - ~39% in `comparison_async_single_listener` (1.96 us -> 1.19 us)
    - ~53% in `comparison_async_fanout_10` (19.16 us -> 8.93 us)
    - ~82% in `comparison_contention_4_publishers` (1.83 us -> 0.33 us)

## Throughput Optimization Update (0.3.6)

After introducing async fanout batching for multi-listener dispatch (with a
single-listener fast path retained), the comparison benchmark reports:

| Group                                | Case           | Before | After  | Delta |
|--------------------------------------|----------------|-------:|-------:|------:|
| `comparison_async_single_listener`   | `jaeb_publish` | 1.17 us | 1.19 us | ~flat |
| `comparison_async_fanout_10`         | `jaeb_publish` | 8.75 us | 4.65 us | ~47% faster |
| `comparison_contention_4_publishers` | `jaeb_publish` | 0.33 us | 0.33 us | ~flat |

Interpretation:

- The optimization targets generalized fanout throughput and substantially
  reduces per-publish spawn overhead when multiple async listeners are
  registered for an event type.
- Single-listener and contention workloads remain essentially unchanged.

## Additional Coverage (0.3.6)

The comparison suite now also includes:

- `comparison_async_fanout_25`
- `comparison_async_fanout_50`
- `comparison_mixed_fanout_multi_type`

Latest measured JAEB times:

| Group                                | Case                      | Time    |
|--------------------------------------|---------------------------|---------:|
| `comparison_async_fanout_25`         | `jaeb_publish`            | 12.17 us |
| `comparison_async_fanout_50`         | `jaeb_publish`            | 22.50 us |
| `comparison_mixed_fanout_multi_type` | `jaeb_publish_mixed`      | 1.71 us  |

## evno Contention Caveat

`evno` is intentionally **not** included in `comparison_contention_4_publishers`.

### Root cause: gyre 1.1.4 `ClaimGuard::Drop` notification race

evno 1.0.2 uses [gyre 1.1.4](https://crates.io/crates/gyre) as its ring-buffer backend. All publishers
serialize through a `Fence` (async spin lock) via `claim_slot()`. Publisher hand-off uses `Notify::notify_one()`
inside `ClaimGuard::Drop`:

```rust
// gyre-1.1.4/src/publisher.rs
struct ClaimGuard<'a>(i64, fence::Guard<'a>, &'a SequenceController);

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        self.2.publisher_notify.notify_one(); // ← fires FIRST
    }
}
// Rust then drops fields in declaration order:
//   i64  →  fence::Guard (releases Fence lock)  →  &ref
```

`notify_one()` wakes exactly one waiting publisher **before** the `fence::Guard` is dropped (which releases the
Fence lock). The woken publisher calls `try_acquire()`, finds the Fence still held, and goes back to sleep — the
notification is lost. Meanwhile the original publisher's guard drops, freeing the Fence, but no one is notified.

Under 4 concurrent publishers this causes **terminal starvation**: when a publisher finishes its iteration batch,
its final `notify_one()` wakes only one peer; the remaining N-2 publishers stay blocked on `Notify::notified()`
forever.

### Workaround attempts

| Strategy                       | Result                                                                 |
|--------------------------------|------------------------------------------------------------------------|
| `yield_now()` every 64 emits  | Hangs during criterion warmup                                          |
| `yield_now()` after every emit | Mostly works but still triggers ~2% timeout hits across 100 samples    |
| Per-iteration 30s timeout      | Prevents process hang but does not fix the underlying starvation       |

The `yield_now()` after every emit approach adds ~50–200 ns overhead per emit and still cannot fully eliminate the
race. A standalone benchmark (`benches/evno_contention.rs`) is included for reproducing and re-evaluating if gyre
releases a fix.

### Conclusion

The deadlock is in gyre's publisher hand-off logic, not in the benchmark code. Until gyre addresses the
`ClaimGuard::Drop` ordering (notify after guard release, or use `notify_waiters()` instead of `notify_one()`),
the evno contention case remains excluded from the comparison suite to keep benchmark runs deterministic and
CI-friendly.
