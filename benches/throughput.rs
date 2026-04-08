// SPDX-License-Identifier: MIT
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};

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

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_publish_sync(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("publish_sync_1_handler", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let bus = EventBus::new(1024);
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
            let bus = EventBus::new(1024);
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
                let bus = EventBus::new(1024);
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
            let bus = EventBus::new(65536);
            let start = std::time::Instant::now();
            for i in 0..iters {
                let _ = bus.try_publish(LightEvent(i));
            }
            bus.shutdown().await.unwrap();
            start.elapsed()
        });
    });
}

criterion_group!(benches, bench_publish_sync, bench_publish_async, bench_fanout, bench_try_publish);
criterion_main!(benches);
