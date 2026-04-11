use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use jaeb::{EventBus, EventHandler, HandlerResult, RetryStrategy, SubscriptionPolicy, SyncEventHandler};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Job;

// ── Handlers ─────────────────────────────────────────────────────────

struct FailOnceAsync {
    attempts: Arc<AtomicUsize>,
}

impl EventHandler<Job> for FailOnceAsync {
    async fn handle(&self, _event: &Job) -> HandlerResult {
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        if n == 0 { Err("first attempt fails".into()) } else { Ok(()) }
    }
}

struct AlwaysFailSync {
    attempts: Arc<AtomicUsize>,
}

impl SyncEventHandler<Job> for AlwaysFailSync {
    fn handle(&self, _event: &Job) -> HandlerResult {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err("always fails".into())
    }
}

struct FailNTimesAsync {
    attempts: Arc<AtomicUsize>,
    fail_count: usize,
}

impl EventHandler<Job> for FailNTimesAsync {
    async fn handle(&self, _event: &Job) -> HandlerResult {
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            Err(format!("attempt {n} fails").into())
        } else {
            Ok(())
        }
    }
}

struct AlwaysFailAsync {
    attempts: Arc<AtomicUsize>,
}

impl EventHandler<Job> for AlwaysFailAsync {
    async fn handle(&self, _event: &Job) -> HandlerResult {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err("always fails".into())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_policy_retries_async_handler() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    let policy = SubscriptionPolicy::default()
        .with_max_retries(1)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(1)));

    let _ = bus
        .subscribe_with_policy(
            FailOnceAsync {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(Job).await.expect("publish");

    // Shutdown drains in-flight async handlers (including retries).
    bus.shutdown().await.expect("shutdown");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn sync_handler_single_attempt_on_failure() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    // Subscribe with default policy (max_retries: 0) — should succeed.
    let _ = bus
        .subscribe(AlwaysFailSync {
            attempts: Arc::clone(&attempts),
        })
        .await
        .expect("subscribe");

    bus.publish(Job).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    // Sync handler executes exactly once — no retries.
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_multiple_attempts() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    // Fail the first 3, succeed on the 4th → need max_retries=3.
    let policy = SubscriptionPolicy::default()
        .with_max_retries(3)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(1)));

    let _ = bus
        .subscribe_with_policy(
            FailNTimesAsync {
                attempts: Arc::clone(&attempts),
                fail_count: 3,
            },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(Job).await.expect("publish");

    // Shutdown drains in-flight async handlers (including retries).
    bus.shutdown().await.expect("shutdown");
    assert_eq!(attempts.load(Ordering::SeqCst), 4); // 1 initial + 3 retries
}

#[tokio::test]
async fn retry_delay_is_respected() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    let policy = SubscriptionPolicy::default()
        .with_max_retries(1)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(50)))
        .with_dead_letter(false);

    let _ = bus
        .subscribe_with_policy(
            AlwaysFailAsync {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    let start = Instant::now();
    bus.publish(Job).await.expect("publish");

    // Shutdown waits for in-flight tasks (including retry delays).
    bus.shutdown().await.expect("shutdown");
    let elapsed = start.elapsed();

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    // With a 50ms retry delay and 1 retry, the total should be at least 50ms.
    assert!(elapsed >= Duration::from_millis(45), "expected >= 45ms, got {elapsed:?}");
}

// ── RetryStrategy tests ──────────────────────────────────────────────

#[tokio::test]
async fn retry_exponential_backoff() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    // Exponential: base=25ms, max=200ms → delays: 25ms, 50ms (attempts 0,1)
    let policy = SubscriptionPolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Exponential {
            base: Duration::from_millis(25),
            max: Duration::from_millis(200),
        })
        .with_dead_letter(false);

    let _ = bus
        .subscribe_with_policy(
            AlwaysFailAsync {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    let start = Instant::now();
    bus.publish(Job).await.expect("publish");
    bus.shutdown().await.expect("shutdown");
    let elapsed = start.elapsed();

    assert_eq!(attempts.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
    // Total delay should be at least 25 + 50 = 75ms (with some tolerance).
    assert!(elapsed >= Duration::from_millis(65), "expected >= 65ms, got {elapsed:?}");
}

#[tokio::test]
async fn retry_exponential_caps_at_max() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    // Exponential: base=50ms, max=60ms → delays: 50ms, 60ms (capped), 60ms (capped)
    let policy = SubscriptionPolicy::default()
        .with_max_retries(3)
        .with_retry_strategy(RetryStrategy::Exponential {
            base: Duration::from_millis(50),
            max: Duration::from_millis(60),
        })
        .with_dead_letter(false);

    let _ = bus
        .subscribe_with_policy(
            AlwaysFailAsync {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    let start = Instant::now();
    bus.publish(Job).await.expect("publish");
    bus.shutdown().await.expect("shutdown");
    let elapsed = start.elapsed();

    assert_eq!(attempts.load(Ordering::SeqCst), 4); // 1 initial + 3 retries
    // Total delay: 50 + 60 + 60 = 170ms. Allow some tolerance.
    assert!(elapsed >= Duration::from_millis(150), "expected >= 150ms, got {elapsed:?}");
    // Should not be wildly over the cap either.
    assert!(elapsed < Duration::from_millis(500), "expected < 500ms, got {elapsed:?}");
}

#[tokio::test]
async fn retry_exponential_with_jitter_is_bounded() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    // Jitter: base=10ms, max=100ms → delays are random in [0, capped]
    let policy = SubscriptionPolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::ExponentialWithJitter {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
        })
        .with_dead_letter(false);

    let _ = bus
        .subscribe_with_policy(
            AlwaysFailAsync {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    let start = Instant::now();
    bus.publish(Job).await.expect("publish");
    bus.shutdown().await.expect("shutdown");
    let elapsed = start.elapsed();

    assert_eq!(attempts.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
    // Jittered delays are in [0, capped], so total could be near zero.
    // Upper bound: 10 + 20 = 30ms max before jitter, so total < 100ms easily.
    assert!(elapsed < Duration::from_millis(500), "expected < 500ms, got {elapsed:?}");
}

#[tokio::test]
async fn retry_no_strategy_retries_immediately() {
    let bus = EventBus::new(16).expect("valid config");
    let attempts = Arc::new(AtomicUsize::new(0));

    // max_retries with no strategy → retries happen immediately (no delay).
    let policy = SubscriptionPolicy::default().with_max_retries(2).with_dead_letter(false);

    let _ = bus
        .subscribe_with_policy(
            AlwaysFailAsync {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    let start = Instant::now();
    bus.publish(Job).await.expect("publish");
    bus.shutdown().await.expect("shutdown");
    let elapsed = start.elapsed();

    assert_eq!(attempts.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
    // No delay strategy → should complete very fast.
    assert!(elapsed < Duration::from_millis(100), "expected < 100ms, got {elapsed:?}");
}

#[test]
fn retry_strategy_delay_for_attempt_fixed() {
    let strategy = RetryStrategy::Fixed(Duration::from_millis(42));
    assert_eq!(strategy.delay_for_attempt(0), Duration::from_millis(42));
    assert_eq!(strategy.delay_for_attempt(1), Duration::from_millis(42));
    assert_eq!(strategy.delay_for_attempt(10), Duration::from_millis(42));
}

#[test]
fn retry_strategy_delay_for_attempt_exponential() {
    let strategy = RetryStrategy::Exponential {
        base: Duration::from_millis(10),
        max: Duration::from_millis(100),
    };
    assert_eq!(strategy.delay_for_attempt(0), Duration::from_millis(10)); // 10 * 2^0
    assert_eq!(strategy.delay_for_attempt(1), Duration::from_millis(20)); // 10 * 2^1
    assert_eq!(strategy.delay_for_attempt(2), Duration::from_millis(40)); // 10 * 2^2
    assert_eq!(strategy.delay_for_attempt(3), Duration::from_millis(80)); // 10 * 2^3
    assert_eq!(strategy.delay_for_attempt(4), Duration::from_millis(100)); // capped at max
    assert_eq!(strategy.delay_for_attempt(10), Duration::from_millis(100)); // still capped
}

#[test]
fn retry_strategy_delay_for_attempt_jitter_bounded() {
    let strategy = RetryStrategy::ExponentialWithJitter {
        base: Duration::from_millis(10),
        max: Duration::from_millis(100),
    };
    // Run several attempts and verify each is bounded.
    for attempt in 0..10 {
        let delay = strategy.delay_for_attempt(attempt);
        let exp_cap = std::cmp::min(
            Duration::from_millis(10).saturating_mul(1u32.checked_shl(attempt as u32).unwrap_or(u32::MAX)),
            Duration::from_millis(100),
        );
        assert!(delay <= exp_cap, "attempt {attempt}: {delay:?} > cap {exp_cap:?}");
    }
}
