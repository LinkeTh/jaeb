use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use criterion::{Criterion, criterion_group, criterion_main};
use eventador::Eventador;
use eventbuzz::asynchronous::prelude::{ApplicationEvent, AsyncApplicationEventListener, AsyncEventbus};
use evno::{Bus as EvnoBus, Close as EvnoClose, Emit as EvnoEmit, Guard as EvnoGuard, from_fn as evno_from_fn};
use jaeb::{EventBus, EventHandler, HandlerResult};

#[derive(Clone)]
struct JaebEvent(#[allow(dead_code)] u64);

#[derive(Clone)]
struct JaebEventAlt(#[allow(dead_code)] u64);

struct JaebNoOp;
struct JaebNoOpAlt;

impl EventHandler<JaebEvent> for JaebNoOp {
    async fn handle(&self, _event: &JaebEvent) -> HandlerResult {
        Ok(())
    }
}

impl EventHandler<JaebEventAlt> for JaebNoOpAlt {
    async fn handle(&self, _event: &JaebEventAlt) -> HandlerResult {
        Ok(())
    }
}

#[derive(Clone)]
struct EventbuzzEvent(#[allow(dead_code)] u64);

#[derive(Clone)]
struct EventbuzzEventAlt(#[allow(dead_code)] u64);

impl ApplicationEvent for EventbuzzEvent {}
impl ApplicationEvent for EventbuzzEventAlt {}

struct EventbuzzNoOp;

#[async_trait]
impl AsyncApplicationEventListener<EventbuzzEvent> for EventbuzzNoOp {
    async fn on_application_event(&self, _event: &EventbuzzEvent) {}
}

#[async_trait]
impl AsyncApplicationEventListener<EventbuzzEventAlt> for EventbuzzNoOp {
    async fn on_application_event(&self, _event: &EventbuzzEventAlt) {}
}

#[derive(Clone, Debug)]
struct EvnoEvent(#[allow(dead_code)] u64);

#[derive(Clone, Debug)]
struct EvnoEventAlt(#[allow(dead_code)] u64);

fn bench_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("create runtime")
}

fn bench_async_single_listener(c: &mut Criterion) {
    let rt = bench_runtime();
    let mut group = c.benchmark_group("comparison_async_single_listener");

    group.bench_function("jaeb_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::builder().buffer_size(1024).build().await.expect("valid config");
            let _sub = bus.subscribe::<JaebEvent, _, _>(JaebNoOp).await.expect("subscribe");

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(JaebEvent(i)).await.expect("publish");
            }
            let elapsed = start.elapsed();

            bus.shutdown().await.expect("shutdown");
            elapsed
        })
    });

    group.bench_function("eventbuzz_publish_event", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut bus: AsyncEventbus = AsyncEventbus::builder().build();
            bus.register_listener::<EventbuzzEvent, _>(EventbuzzNoOp).await;

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish_event(EventbuzzEvent(i)).await;
            }
            start.elapsed()
        })
    });

    group.bench_function("evno_emit", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EvnoBus::new(1024);
            let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEvent>| async move {}));

            tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEvent(u64::MAX)))
                .await
                .expect("evno listener startup timeout");

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.emit(EvnoEvent(i)).await;
                if i % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            let elapsed = start.elapsed();

            tokio::time::timeout(Duration::from_secs(5), bus.close())
                .await
                .expect("evno close timeout");
            elapsed
        })
    });

    group.bench_function("eventador_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = Eventador::new(1024).expect("create eventador");
            let sub = bus.subscribe::<u64>();
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = Arc::clone(&stop);

            let drainer = std::thread::spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    let _ = sub.recv();
                }
            });

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(i);
            }
            let elapsed = start.elapsed();

            stop.store(true, Ordering::Relaxed);
            bus.publish(0u64); // sentinel to wake blocked recv
            drainer.join().expect("eventador drainer join");
            elapsed
        })
    });

    group.finish();
}

fn bench_async_fanout_10(c: &mut Criterion) {
    let rt = bench_runtime();
    let mut group = c.benchmark_group("comparison_async_fanout_10");

    group.bench_function("jaeb_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::builder().buffer_size(1024).build().await.expect("valid config");
            for _ in 0..10 {
                let _sub = bus.subscribe::<JaebEvent, _, _>(JaebNoOp).await.expect("subscribe");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(JaebEvent(i)).await.expect("publish");
            }
            let elapsed = start.elapsed();

            bus.shutdown().await.expect("shutdown");
            elapsed
        })
    });

    group.bench_function("eventbuzz_publish_event", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut bus: AsyncEventbus = AsyncEventbus::builder().build();
            for _ in 0..10 {
                bus.register_listener::<EventbuzzEvent, _>(EventbuzzNoOp).await;
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish_event(EventbuzzEvent(i)).await;
            }
            start.elapsed()
        })
    });

    group.bench_function("evno_emit", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EvnoBus::new(1024);
            for i in 0..10 {
                let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEvent>| async move {}));

                // Work around an upstream evno startup race by forcing each bind
                // to complete before registering the next listener.
                tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEvent(u64::MAX - i as u64)))
                    .await
                    .expect("evno per-listener startup timeout");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.emit(EvnoEvent(i)).await;
                if i % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            let elapsed = start.elapsed();

            tokio::time::timeout(Duration::from_secs(5), bus.close())
                .await
                .expect("evno close timeout");
            elapsed
        })
    });

    group.bench_function("eventador_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = Eventador::new(1024).expect("create eventador");
            let stop = Arc::new(AtomicBool::new(false));
            let mut drainers = Vec::with_capacity(10);
            for _ in 0..10 {
                let sub = bus.subscribe::<u64>();
                let stop2 = Arc::clone(&stop);
                drainers.push(std::thread::spawn(move || {
                    while !stop2.load(Ordering::Relaxed) {
                        let _ = sub.recv();
                    }
                }));
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(i);
            }
            let elapsed = start.elapsed();

            stop.store(true, Ordering::Relaxed);
            bus.publish(0u64); // sentinel to wake blocked recv
            for d in drainers {
                d.join().expect("eventador drainer join");
            }
            elapsed
        })
    });

    group.finish();
}

fn bench_async_fanout_25(c: &mut Criterion) {
    let rt = bench_runtime();
    let mut group = c.benchmark_group("comparison_async_fanout_25");

    group.bench_function("jaeb_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::builder().buffer_size(1024).build().await.expect("valid config");
            for _ in 0..25 {
                let _sub = bus.subscribe::<JaebEvent, _, _>(JaebNoOp).await.expect("subscribe");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(JaebEvent(i)).await.expect("publish");
            }
            let elapsed = start.elapsed();

            bus.shutdown().await.expect("shutdown");
            elapsed
        })
    });

    group.bench_function("eventbuzz_publish_event", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut bus: AsyncEventbus = AsyncEventbus::builder().build();
            for _ in 0..25 {
                bus.register_listener::<EventbuzzEvent, _>(EventbuzzNoOp).await;
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish_event(EventbuzzEvent(i)).await;
            }
            start.elapsed()
        })
    });

    group.bench_function("evno_emit", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EvnoBus::new(1024);
            for i in 0..25 {
                let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEvent>| async move {}));

                tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEvent(u64::MAX - i as u64)))
                    .await
                    .expect("evno per-listener startup timeout");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.emit(EvnoEvent(i)).await;
                if i % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            let elapsed = start.elapsed();

            tokio::time::timeout(Duration::from_secs(5), bus.close())
                .await
                .expect("evno close timeout");
            elapsed
        })
    });

    group.bench_function("eventador_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = Eventador::new(1024).expect("create eventador");
            let stop = Arc::new(AtomicBool::new(false));
            let mut drainers = Vec::with_capacity(25);
            for _ in 0..25 {
                let sub = bus.subscribe::<u64>();
                let stop2 = Arc::clone(&stop);
                drainers.push(std::thread::spawn(move || {
                    while !stop2.load(Ordering::Relaxed) {
                        let _ = sub.recv();
                    }
                }));
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(i);
            }
            let elapsed = start.elapsed();

            stop.store(true, Ordering::Relaxed);
            bus.publish(0u64);
            for d in drainers {
                d.join().expect("eventador drainer join");
            }
            elapsed
        })
    });

    group.finish();
}

fn bench_async_fanout_50(c: &mut Criterion) {
    let rt = bench_runtime();
    let mut group = c.benchmark_group("comparison_async_fanout_50");

    group.bench_function("jaeb_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::builder().buffer_size(1024).build().await.expect("valid config");
            for _ in 0..50 {
                let _sub = bus.subscribe::<JaebEvent, _, _>(JaebNoOp).await.expect("subscribe");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(JaebEvent(i)).await.expect("publish");
            }
            let elapsed = start.elapsed();

            bus.shutdown().await.expect("shutdown");
            elapsed
        })
    });

    group.bench_function("eventbuzz_publish_event", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut bus: AsyncEventbus = AsyncEventbus::builder().build();
            for _ in 0..50 {
                bus.register_listener::<EventbuzzEvent, _>(EventbuzzNoOp).await;
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish_event(EventbuzzEvent(i)).await;
            }
            start.elapsed()
        })
    });

    group.bench_function("evno_emit", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EvnoBus::new(1024);
            for i in 0..50 {
                let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEvent>| async move {}));

                tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEvent(u64::MAX - i as u64)))
                    .await
                    .expect("evno per-listener startup timeout");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.emit(EvnoEvent(i)).await;
                if i % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            let elapsed = start.elapsed();

            tokio::time::timeout(Duration::from_secs(5), bus.close())
                .await
                .expect("evno close timeout");
            elapsed
        })
    });

    group.bench_function("eventador_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = Eventador::new(1024).expect("create eventador");
            let stop = Arc::new(AtomicBool::new(false));
            let mut drainers = Vec::with_capacity(50);
            for _ in 0..50 {
                let sub = bus.subscribe::<u64>();
                let stop2 = Arc::clone(&stop);
                drainers.push(std::thread::spawn(move || {
                    while !stop2.load(Ordering::Relaxed) {
                        let _ = sub.recv();
                    }
                }));
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(i);
            }
            let elapsed = start.elapsed();

            stop.store(true, Ordering::Relaxed);
            bus.publish(0u64);
            for d in drainers {
                d.join().expect("eventador drainer join");
            }
            elapsed
        })
    });

    group.finish();
}

fn bench_mixed_fanout_multi_type(c: &mut Criterion) {
    let rt = bench_runtime();
    let mut group = c.benchmark_group("comparison_mixed_fanout_multi_type");

    group.bench_function("jaeb_publish_mixed", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::builder().buffer_size(1024).build().await.expect("valid config");
            for _ in 0..5 {
                let _sub = bus.subscribe::<JaebEvent, _, _>(JaebNoOp).await.expect("subscribe jaeb");
            }
            for _ in 0..5 {
                let _sub = bus.subscribe::<JaebEventAlt, _, _>(JaebNoOpAlt).await.expect("subscribe jaeb alt");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                if i % 2 == 0 {
                    bus.publish(JaebEvent(i)).await.expect("publish jaeb");
                } else {
                    bus.publish(JaebEventAlt(i)).await.expect("publish jaeb alt");
                }
            }
            let elapsed = start.elapsed();

            bus.shutdown().await.expect("shutdown");
            elapsed
        })
    });

    group.bench_function("eventbuzz_publish_mixed", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut bus: AsyncEventbus = AsyncEventbus::builder().build();
            for _ in 0..5 {
                bus.register_listener::<EventbuzzEvent, _>(EventbuzzNoOp).await;
            }
            for _ in 0..5 {
                bus.register_listener::<EventbuzzEventAlt, _>(EventbuzzNoOp).await;
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                if i % 2 == 0 {
                    bus.publish_event(EventbuzzEvent(i)).await;
                } else {
                    bus.publish_event(EventbuzzEventAlt(i)).await;
                }
            }
            start.elapsed()
        })
    });

    group.bench_function("evno_emit_mixed", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EvnoBus::new(1024);
            for i in 0..5 {
                let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEvent>| async move {}));
                tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEvent(u64::MAX - i as u64)))
                    .await
                    .expect("evno startup timeout event a");
            }
            for i in 0..5 {
                let _sub = bus.on(evno_from_fn(|_event: EvnoGuard<EvnoEventAlt>| async move {}));
                tokio::time::timeout(Duration::from_secs(2), bus.emit(EvnoEventAlt(u64::MAX - i as u64)))
                    .await
                    .expect("evno startup timeout event b");
            }

            let start = std::time::Instant::now();
            for i in 0..iters {
                if i % 2 == 0 {
                    bus.emit(EvnoEvent(i)).await;
                } else {
                    bus.emit(EvnoEventAlt(i)).await;
                }
                if i % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            let elapsed = start.elapsed();

            tokio::time::timeout(Duration::from_secs(5), bus.close())
                .await
                .expect("evno close timeout");
            elapsed
        })
    });

    group.bench_function("eventador_publish_mixed", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = Eventador::new(1024).expect("create eventador");
            let stop = Arc::new(AtomicBool::new(false));

            let sub_a = bus.subscribe::<u64>();
            let stop_a = Arc::clone(&stop);
            let drainer_a = std::thread::spawn(move || {
                while !stop_a.load(Ordering::Relaxed) {
                    let _ = sub_a.recv();
                }
            });

            let sub_b = bus.subscribe::<u32>();
            let stop_b = Arc::clone(&stop);
            let drainer_b = std::thread::spawn(move || {
                while !stop_b.load(Ordering::Relaxed) {
                    let _ = sub_b.recv();
                }
            });

            let start = std::time::Instant::now();
            for i in 0..iters {
                if i % 2 == 0 {
                    bus.publish(i);
                } else {
                    bus.publish(i as u32);
                }
            }
            let elapsed = start.elapsed();

            stop.store(true, Ordering::Relaxed);
            bus.publish(0u64);
            bus.publish(0u32);
            drainer_a.join().expect("eventador drainer a join");
            drainer_b.join().expect("eventador drainer b join");
            elapsed
        })
    });

    group.finish();
}

fn bench_contention_4_publishers(c: &mut Criterion) {
    let rt = bench_runtime();
    let mut group = c.benchmark_group("comparison_contention_4_publishers");

    group.bench_function("jaeb_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let workers = 4usize;
            let per = (iters as usize) / workers;
            let extra = (iters as usize) % workers;

            let bus = EventBus::builder().buffer_size(4096).build().await.expect("valid config");
            let _sub = bus.subscribe::<JaebEvent, _, _>(JaebNoOp).await.expect("subscribe");

            let start = std::time::Instant::now();
            let mut joins = Vec::with_capacity(workers);
            for worker_idx in 0..workers {
                let bus = bus.clone();
                let n = per + usize::from(worker_idx < extra);
                joins.push(tokio::spawn(async move {
                    for i in 0..n {
                        bus.publish(JaebEvent(i as u64)).await.expect("publish");
                    }
                }));
            }
            for join in joins {
                join.await.expect("join");
            }
            let elapsed = start.elapsed();

            bus.shutdown().await.expect("shutdown");
            elapsed
        })
    });

    group.bench_function("eventbuzz_publish_event", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let workers = 4usize;
            let per = (iters as usize) / workers;
            let extra = (iters as usize) % workers;

            let mut bus: AsyncEventbus = AsyncEventbus::builder().build();
            bus.register_listener::<EventbuzzEvent, _>(EventbuzzNoOp).await;
            let bus = Arc::new(bus);

            let start = std::time::Instant::now();
            let mut joins = Vec::with_capacity(workers);
            for worker_idx in 0..workers {
                let bus = Arc::clone(&bus);
                let n = per + usize::from(worker_idx < extra);
                joins.push(tokio::spawn(async move {
                    for i in 0..n {
                        bus.publish_event(EventbuzzEvent(i as u64)).await;
                    }
                }));
            }
            for join in joins {
                join.await.expect("join");
            }
            start.elapsed()
        })
    });

    // NOTE: evno contention benchmark intentionally omitted due to a race
    // condition in gyre 1.1.4 (evno's ring-buffer backend).
    //
    // Root cause: `ClaimGuard::Drop` in gyre's publisher.rs calls
    // `notify_one()` BEFORE the `fence::Guard` (Fence lock) is dropped.
    // Under concurrent publishers this causes terminal starvation — woken
    // publishers find the Fence still held, go back to sleep, and the
    // notification is lost. When a publisher finishes its iterations, only
    // one peer is woken; remaining publishers block forever.
    //
    // Workaround attempts (yield_now after every emit, per-iteration
    // timeouts) reduce the probability but do not eliminate it — the
    // standalone benchmark in `benches/evno_contention.rs` still triggers
    // timeouts ~2% of samples.

    group.bench_function("eventador_publish", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let workers = 4usize;
            let per = (iters as usize) / workers;
            let extra = (iters as usize) % workers;

            let bus = Eventador::new(4096).expect("create eventador");
            let sub = bus.subscribe::<u64>();
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = Arc::clone(&stop);

            let drainer = std::thread::spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    let _ = sub.recv();
                }
            });

            let start = std::time::Instant::now();
            let mut joins = Vec::with_capacity(workers);
            for worker_idx in 0..workers {
                let bus = bus.clone();
                let n = per + usize::from(worker_idx < extra);
                joins.push(tokio::spawn(async move {
                    for i in 0..n {
                        bus.publish(i as u64);
                    }
                }));
            }
            for join in joins {
                join.await.expect("join");
            }
            let elapsed = start.elapsed();

            stop.store(true, Ordering::Relaxed);
            bus.publish(0u64); // sentinel to wake blocked recv
            drainer.join().expect("eventador drainer join");
            elapsed
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_async_single_listener,
    bench_async_fanout_10,
    bench_async_fanout_25,
    bench_async_fanout_50,
    bench_mixed_fanout_multi_type,
    bench_contention_4_publishers,
);
criterion_main!(benches);
