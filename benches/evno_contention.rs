/// Standalone benchmark isolating evno's 4-publisher contention case.
///
/// This exists separately from `comparison.rs` so we can iterate quickly without
/// running the full comparison suite. The benchmark includes a per-iteration
/// timeout as a safety net against the gyre publisher starvation bug.
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use evno::{Bus as EvnoBus, Close as EvnoClose, Emit as EvnoEmit, Guard as EvnoGuard, from_fn as evno_from_fn};

#[derive(Clone, Debug)]
struct EvnoEvent(#[allow(dead_code)] u64);

fn bench_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("create runtime")
}

fn bench_evno_contention(c: &mut Criterion) {
    let rt = bench_runtime();
    let mut group = c.benchmark_group("evno_contention_4_publishers");

    // Workaround strategy: yield after every emit to give gyre's Fence a chance
    // to be released before the woken publisher tries to acquire it.
    // A per-iteration timeout (30s) acts as a safety net so the benchmark process
    // does not hang forever if the gyre starvation bug triggers.
    group.bench_function("evno_emit_yield_every", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let workers = 4usize;
            let per = (iters as usize) / workers;
            let extra = (iters as usize) % workers;

            let bus = EvnoBus::new(4096);
            let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEvent>| async move {}));

            // Warm up: ensure the listener is bound before timing starts.
            tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEvent(u64::MAX)))
                .await
                .expect("evno listener startup timeout");

            let start = std::time::Instant::now();

            let result = tokio::time::timeout(Duration::from_secs(30), async {
                let mut joins = Vec::with_capacity(workers);
                for worker_idx in 0..workers {
                    let bus = bus.clone();
                    let n = per + usize::from(worker_idx < extra);
                    joins.push(tokio::spawn(async move {
                        for i in 0..n {
                            bus.emit(EvnoEvent(i as u64)).await;
                            // Yield after every emit to probabilistically mitigate
                            // the gyre ClaimGuard::Drop notification race.
                            tokio::task::yield_now().await;
                        }
                    }));
                }
                for join in joins {
                    join.await.expect("join");
                }
            })
            .await;

            let elapsed = start.elapsed();

            if result.is_err() {
                eprintln!(
                    "WARNING: evno contention benchmark timed out after 30s \
                     (gyre publisher starvation bug). Returning elapsed time as-is."
                );
            }

            tokio::time::timeout(Duration::from_secs(5), bus.close())
                .await
                .expect("evno close timeout");

            elapsed
        })
    });

    // Variant: yield every 64 emits (original strategy from comparison.rs).
    // Less overhead but more likely to trigger the starvation bug.
    group.bench_function("evno_emit_yield_every_64", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let workers = 4usize;
            let per = (iters as usize) / workers;
            let extra = (iters as usize) % workers;

            let bus = EvnoBus::new(4096);
            let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEvent>| async move {}));

            tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEvent(u64::MAX)))
                .await
                .expect("evno listener startup timeout");

            let start = std::time::Instant::now();

            let result = tokio::time::timeout(Duration::from_secs(30), async {
                let mut joins = Vec::with_capacity(workers);
                for worker_idx in 0..workers {
                    let bus = bus.clone();
                    let n = per + usize::from(worker_idx < extra);
                    joins.push(tokio::spawn(async move {
                        for i in 0..n {
                            bus.emit(EvnoEvent(i as u64)).await;
                            if i % 64 == 0 {
                                tokio::task::yield_now().await;
                            }
                        }
                    }));
                }
                for join in joins {
                    join.await.expect("join");
                }
            })
            .await;

            let elapsed = start.elapsed();

            if result.is_err() {
                eprintln!(
                    "WARNING: evno contention benchmark (yield/64) timed out after 30s \
                     (gyre publisher starvation bug). Returning elapsed time as-is."
                );
            }

            tokio::time::timeout(Duration::from_secs(5), bus.close())
                .await
                .expect("evno close timeout");

            elapsed
        })
    });

    group.finish();
}

criterion_group!(benches, bench_evno_contention);
criterion_main!(benches);
