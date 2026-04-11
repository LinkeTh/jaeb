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
- `comparison_contention_4_publishers`: four concurrent publishers, one async no-op listener

## Latest Measurements (median-ish criterion time window)

| Group                                | Case                      |    Time |
|--------------------------------------|---------------------------|--------:|
| `comparison_async_single_listener`   | `jaeb_publish`            | 1.39 us |
| `comparison_async_single_listener`   | `eventbuzz_publish_event` | 0.94 us |
| `comparison_async_single_listener`   | `evno_emit`               | 0.56 us |
| `comparison_async_fanout_10`         | `jaeb_publish`            | 10.64 us |
| `comparison_async_fanout_10`         | `eventbuzz_publish_event` | 6.87 us |
| `comparison_async_fanout_10`         | `evno_emit`               | 4.64 us |
| `comparison_contention_4_publishers` | `jaeb_publish`            | 0.45 us |
| `comparison_contention_4_publishers` | `eventbuzz_publish_event` | 0.26 us |

Notes:

- Criterion reports a confidence interval, not a single exact scalar. Values above are rounded from the reported central range.
- These numbers are for relative orientation only, not absolute SLA guarantees.
- Relative to the prior run documented in this file, `jaeb` improved by roughly:
    - ~29% in `comparison_async_single_listener`
    - ~45% in `comparison_async_fanout_10`
    - ~75% in `comparison_contention_4_publishers`

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
