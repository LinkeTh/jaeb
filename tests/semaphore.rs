use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{AsyncSubscriptionPolicy, EventBus, EventHandler, HandlerResult};
use tokio::sync::{oneshot, watch};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Ping;

#[derive(Clone, Debug)]
struct Pong;

// ── Handlers ─────────────────────────────────────────────────────────

struct SlowHandler {
    count: Arc<AtomicUsize>,
}

impl EventHandler<Ping> for SlowHandler {
    async fn handle(&self, _event: &Ping, _bus: &EventBus) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct PeakHandler {
    current: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl EventHandler<Ping> for PeakHandler {
    async fn handle(&self, _event: &Ping, _bus: &EventBus) -> HandlerResult {
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(40)).await;
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

impl EventHandler<Pong> for PeakHandler {
    async fn handle(&self, _event: &Pong, _bus: &EventBus) -> HandlerResult {
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(40)).await;
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Hot;

#[derive(Clone, Debug)]
struct Cold;

#[derive(Clone, Debug)]
struct RetryHot;

struct HotBlocker {
    starts: Arc<AtomicUsize>,
    release_rxs: std::sync::Mutex<VecDeque<oneshot::Receiver<()>>>,
    order: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl EventHandler<Hot> for HotBlocker {
    async fn handle(&self, _event: &Hot, _bus: &EventBus) -> HandlerResult {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let rx = self.release_rxs.lock().expect("lock release rx").pop_front().expect("release rx present");
        let _ = rx.await;
        self.order.lock().expect("lock order").push("hot");
        Ok(())
    }
}

struct ColdRecorder {
    starts: Arc<AtomicUsize>,
    order: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl EventHandler<Cold> for ColdRecorder {
    async fn handle(&self, _event: &Cold, _bus: &EventBus) -> HandlerResult {
        self.order.lock().expect("lock order").push("cold");
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct RetryReleasesPermit {
    attempts: Arc<AtomicUsize>,
    phase_tx: watch::Sender<usize>,
    order: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl EventHandler<RetryHot> for RetryReleasesPermit {
    async fn handle(&self, _event: &RetryHot, _bus: &EventBus) -> HandlerResult {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            let _ = self.phase_tx.send(1);
            Err("fail once".into())
        } else {
            self.order.lock().expect("lock order").push("retry");
            let _ = self.phase_tx.send(2);
            Ok(())
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

/// When async concurrency is limited and shutdown aborts waiting tasks,
/// the semaphore path must not panic — it should gracefully skip
/// execution for tasks that cannot acquire a permit.
#[tokio::test]
async fn semaphore_limited_bus_shuts_down_cleanly() {
    let bus = EventBus::builder()
        .max_concurrent_async(1)
        .shutdown_timeout(Duration::from_millis(100))
        .build()
        .await
        .expect("valid config");

    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowHandler { count: Arc::clone(&count) }).await.expect("subscribe");

    // Publish several events — with concurrency limit of 1, most will be
    // queued behind the semaphore.
    for _ in 0..5 {
        bus.publish(Ping).await.expect("publish");
    }

    // Shutdown with a short timeout — this will abort tasks still waiting
    // for a semaphore permit. The important assertion is that no panic occurs.
    let _ = bus.shutdown().await;

    // At least the first handler should have run.
    assert!(count.load(Ordering::SeqCst) >= 1);
}

/// Verify that async handlers behind a semaphore execute correctly
/// under normal (non-shutdown) conditions.
#[tokio::test]
async fn semaphore_limited_handlers_execute_normally() {
    let bus = EventBus::builder().max_concurrent_async(2).build().await.expect("valid config");

    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowHandler { count: Arc::clone(&count) }).await.expect("subscribe");

    for _ in 0..4 {
        bus.publish(Ping).await.expect("publish");
    }

    // Shutdown drains in-flight async handlers deterministically.
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn global_async_limit_is_shared_across_event_types() {
    let bus = EventBus::builder().max_concurrent_async(2).build().await.expect("valid config");

    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe::<Ping, _, _>(PeakHandler {
            current: Arc::clone(&current),
            peak: Arc::clone(&peak),
        })
        .await
        .expect("subscribe ping");
    let _ = bus
        .subscribe::<Pong, _, _>(PeakHandler {
            current: Arc::clone(&current),
            peak: Arc::clone(&peak),
        })
        .await
        .expect("subscribe pong");

    for _ in 0..4 {
        bus.publish(Ping).await.expect("publish ping");
        bus.publish(Pong).await.expect("publish pong");
    }

    bus.shutdown().await.expect("shutdown");

    assert!(peak.load(Ordering::SeqCst) <= 2, "global async limit should be shared across event types");
}

#[tokio::test]
async fn single_listener_can_use_full_async_concurrency_limit() {
    let bus = EventBus::builder().max_concurrent_async(2).build().await.expect("valid config");

    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe::<Ping, _, _>(PeakHandler {
            current: Arc::clone(&current),
            peak: Arc::clone(&peak),
        })
        .await
        .expect("subscribe ping");

    for _ in 0..4 {
        bus.publish(Ping).await.expect("publish ping");
    }

    bus.shutdown().await.expect("shutdown");

    assert_eq!(
        peak.load(Ordering::SeqCst),
        2,
        "single listener should be able to use the full async concurrency limit"
    );
}

#[tokio::test]
async fn queued_cross_type_waiter_makes_progress_before_later_hot_work() {
    let bus = EventBus::builder().max_concurrent_async(1).build().await.expect("valid config");

    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let hot_starts = Arc::new(AtomicUsize::new(0));
    let cold_starts = Arc::new(AtomicUsize::new(0));
    let (release_tx1, release_rx1) = oneshot::channel();
    let (release_tx2, release_rx2) = oneshot::channel();

    let _ = bus
        .subscribe(HotBlocker {
            starts: Arc::clone(&hot_starts),
            release_rxs: std::sync::Mutex::new(VecDeque::from([release_rx1, release_rx2])),
            order: Arc::clone(&order),
        })
        .await
        .expect("subscribe hot");
    let _ = bus
        .subscribe(ColdRecorder {
            starts: Arc::clone(&cold_starts),
            order: Arc::clone(&order),
        })
        .await
        .expect("subscribe cold");

    bus.publish(Hot).await.expect("publish hot 1");
    tokio::time::timeout(Duration::from_secs(2), async {
        while hot_starts.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first hot should start");

    bus.publish(Cold).await.expect("publish cold");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if bus.stats().await.expect("stats").in_flight_async >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cold waiter should be queued");

    bus.publish(Hot).await.expect("publish hot 2");

    release_tx1.send(()).expect("release hot 1");
    tokio::time::timeout(Duration::from_secs(2), async {
        while cold_starts.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cold should start next");
    release_tx2.send(()).expect("release hot 2");
    bus.shutdown().await.expect("shutdown");

    let got = order.lock().expect("lock order").clone();
    assert!(got.len() >= 2, "expected at least two completions, got {got:?}");
    assert_eq!(got[0], "hot");
    assert_eq!(got[1], "cold", "older cross-type waiter should make progress before later hot work");
}

#[tokio::test]
async fn retry_backoff_does_not_monopolize_global_permit() {
    let bus = EventBus::builder().max_concurrent_async(1).build().await.expect("valid config");

    let attempts = Arc::new(AtomicUsize::new(0));
    let (phase_tx, mut phase_rx) = watch::channel(0usize);
    let cold_starts = Arc::new(AtomicUsize::new(0));
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));

    let _ = bus
        .subscribe_with_policy(
            RetryReleasesPermit {
                attempts: Arc::clone(&attempts),
                phase_tx,
                order: Arc::clone(&order),
            },
            AsyncSubscriptionPolicy::default()
                .with_max_retries(1)
                .with_retry_strategy(jaeb::RetryStrategy::Fixed(Duration::from_millis(50)))
                .with_dead_letter(false),
        )
        .await
        .expect("subscribe retry hot");
    let _ = bus
        .subscribe(ColdRecorder {
            starts: Arc::clone(&cold_starts),
            order: Arc::clone(&order),
        })
        .await
        .expect("subscribe cold");

    bus.publish(RetryHot).await.expect("publish retry hot");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if *phase_rx.borrow() >= 1 {
                break;
            }
            phase_rx.changed().await.expect("phase sender alive");
        }
    })
    .await
    .expect("first attempt should fail");

    bus.publish(Cold).await.expect("publish cold");
    tokio::time::timeout(Duration::from_secs(2), async {
        while cold_starts.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cold should start before retry");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if *phase_rx.borrow() >= 2 {
                break;
            }
            phase_rx.changed().await.expect("phase sender alive");
        }
    })
    .await
    .expect("retry should eventually run");

    bus.shutdown().await.expect("shutdown");

    let got = order.lock().expect("lock order").clone();
    assert_eq!(
        got.first().copied(),
        Some("cold"),
        "cold work should run before retry attempt reacquires permit"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}
