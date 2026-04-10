use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use jaeb::{EventBus, EventHandler, HandlerResult, Middleware, MiddlewareDecision, SyncEventHandler};

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LightEvent(#[allow(dead_code)] u64);

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

struct NoOpSync;
impl SyncEventHandler<LightEvent> for NoOpSync {
    fn handle(&self, _event: &LightEvent) -> HandlerResult {
        Ok(())
    }
}

struct NoOpAsync;
impl EventHandler<LightEvent> for NoOpAsync {
    async fn handle(&self, _event: &LightEvent) -> HandlerResult {
        Ok(())
    }
}

struct CountingSync(Arc<AtomicUsize>);
impl SyncEventHandler<LightEvent> for CountingSync {
    fn handle(&self, _event: &LightEvent) -> HandlerResult {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct CountingAsync(Arc<AtomicUsize>);
impl EventHandler<LightEvent> for CountingAsync {
    async fn handle(&self, _event: &LightEvent) -> HandlerResult {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_publish_sync(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("publish_sync_1_handler", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<LightEvent, _, _>(NoOpSync).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(LightEvent(i)).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });
}

fn bench_publish_async(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("publish_async_1_handler", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<LightEvent, _, _>(NoOpAsync).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(LightEvent(i)).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });
}

fn bench_fanout(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    for n in [1usize, 5, 10, 50] {
        c.bench_function(&format!("fanout_sync_{n}_handlers"), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let bus = EventBus::new(1024).expect("valid config");
                let counter = Arc::new(AtomicUsize::new(0));
                for _ in 0..n {
                    let _sub = bus.subscribe::<LightEvent, _, _>(CountingSync(counter.clone())).await.unwrap();
                }
                let start = std::time::Instant::now();
                for i in 0..iters {
                    bus.publish(LightEvent(i)).await.unwrap();
                }
                bus.shutdown().await.unwrap();
                start.elapsed()
            });
        });
    }
}

fn bench_try_publish(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("try_publish_no_listeners", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(65536).expect("valid config");
            let start = std::time::Instant::now();
            for i in 0..iters {
                let _ = bus.try_publish(LightEvent(i));
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });
}

/// Mixed workload: both sync and async handlers on the same event type.
fn bench_mixed_sync_async(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("mixed_1_sync_1_async", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let counter = Arc::new(AtomicUsize::new(0));
            let _sub1 = bus.subscribe::<LightEvent, _, _>(CountingSync(counter.clone())).await.unwrap();
            let _sub2 = bus.subscribe::<LightEvent, _, _>(CountingAsync(counter.clone())).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(LightEvent(i)).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });
}

/// Multiple concurrent publishers sharing a single bus.
fn bench_contention(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("contention_4_publishers", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(4096).expect("valid config");
            let _sub = bus.subscribe::<LightEvent, _, _>(NoOpSync).await.unwrap();
            let per_task = iters / 4;
            let start = std::time::Instant::now();
            let mut handles = Vec::new();
            for task_id in 0..4u64 {
                let bus = bus.clone();
                handles.push(tokio::spawn(async move {
                    for i in 0..per_task {
                        bus.publish(LightEvent(task_id * per_task + i)).await.unwrap();
                    }
                }));
            }
            for h in handles {
                h.await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });
}

// ---------------------------------------------------------------------------
// Baseline: raw tokio mpsc (for comparison with jaeb overhead)
// ---------------------------------------------------------------------------

/// Baseline: raw tokio mpsc channel send + recv for comparison.
fn bench_baseline_mpsc(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("baseline_mpsc_send_recv", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<LightEvent>(1024);
            let consumer = tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let start = std::time::Instant::now();
            for i in 0..iters {
                tx.send(LightEvent(i)).await.unwrap();
            }
            drop(tx);
            consumer.await.unwrap();
            start.elapsed()
        });
    });
}

// ---------------------------------------------------------------------------
// Middleware overhead
// ---------------------------------------------------------------------------

struct PassthroughMiddleware;
impl Middleware for PassthroughMiddleware {
    async fn process(&self, _name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        MiddlewareDecision::Continue
    }
}

fn bench_middleware_overhead(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    for n in [0usize, 1, 5, 10] {
        c.bench_function(&format!("middleware_{n}_layers_sync"), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let bus = EventBus::new(1024).expect("valid config");
                let _sub = bus.subscribe::<LightEvent, _, _>(NoOpSync).await.unwrap();
                for _ in 0..n {
                    let _ = bus.add_middleware(PassthroughMiddleware).await.unwrap();
                }
                let start = std::time::Instant::now();
                for i in 0..iters {
                    bus.publish(LightEvent(i)).await.unwrap();
                }
                bus.shutdown().await.unwrap();
                start.elapsed()
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Event-size scaling
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SmallEvent(#[allow(dead_code)] u64);

#[derive(Clone)]
struct MediumEvent(#[allow(dead_code)] [u8; 256]);

#[derive(Clone)]
struct LargeEvent(#[allow(dead_code)] Vec<u8>);

struct NoOpSyncSmall;
impl SyncEventHandler<SmallEvent> for NoOpSyncSmall {
    fn handle(&self, _event: &SmallEvent) -> HandlerResult {
        Ok(())
    }
}

struct NoOpAsyncSmall;
impl EventHandler<SmallEvent> for NoOpAsyncSmall {
    async fn handle(&self, _event: &SmallEvent) -> HandlerResult {
        Ok(())
    }
}

struct NoOpSyncMedium;
impl SyncEventHandler<MediumEvent> for NoOpSyncMedium {
    fn handle(&self, _event: &MediumEvent) -> HandlerResult {
        Ok(())
    }
}

struct NoOpAsyncMedium;
impl EventHandler<MediumEvent> for NoOpAsyncMedium {
    async fn handle(&self, _event: &MediumEvent) -> HandlerResult {
        Ok(())
    }
}

struct NoOpSyncLarge;
impl SyncEventHandler<LargeEvent> for NoOpSyncLarge {
    fn handle(&self, _event: &LargeEvent) -> HandlerResult {
        Ok(())
    }
}

struct NoOpAsyncLarge;
impl EventHandler<LargeEvent> for NoOpAsyncLarge {
    async fn handle(&self, _event: &LargeEvent) -> HandlerResult {
        Ok(())
    }
}

fn bench_event_size(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Sync handlers — measures dispatch overhead without clone cost
    c.bench_function("event_size_8B_sync", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<SmallEvent, _, _>(NoOpSyncSmall).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(SmallEvent(i)).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });

    c.bench_function("event_size_256B_sync", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<MediumEvent, _, _>(NoOpSyncMedium).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(MediumEvent([i as u8; 256])).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });

    c.bench_function("event_size_4KB_sync", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<LargeEvent, _, _>(NoOpSyncLarge).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(LargeEvent(vec![i as u8; 4096])).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });

    // Async handlers — includes the clone cost per handler invocation
    c.bench_function("event_size_8B_async", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<SmallEvent, _, _>(NoOpAsyncSmall).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(SmallEvent(i)).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });

    c.bench_function("event_size_256B_async", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<MediumEvent, _, _>(NoOpAsyncMedium).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(MediumEvent([i as u8; 256])).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });

    c.bench_function("event_size_4KB_async", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024).expect("valid config");
            let _sub = bus.subscribe::<LargeEvent, _, _>(NoOpAsyncLarge).await.unwrap();
            let start = std::time::Instant::now();
            for i in 0..iters {
                bus.publish(LargeEvent(vec![i as u8; 4096])).await.unwrap();
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });
}

criterion_group!(
    benches,
    bench_publish_sync,
    bench_publish_async,
    bench_fanout,
    bench_try_publish,
    bench_mixed_sync_async,
    bench_contention,
    bench_baseline_mpsc,
    bench_middleware_overhead,
    bench_event_size,
);
criterion_main!(benches);
