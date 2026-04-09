// SPDX-License-Identifier: MIT
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use jaeb::{EventBus, EventHandler, FailurePolicy, HandlerResult, SyncEventHandler};

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

struct FailOnceSync {
    attempts: Arc<AtomicUsize>,
}

impl SyncEventHandler<Job> for FailOnceSync {
    fn handle(&self, _event: &Job) -> HandlerResult {
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        if n == 0 { Err("first attempt fails".into()) } else { Ok(()) }
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
    let bus = EventBus::new(16);
    let attempts = Arc::new(AtomicUsize::new(0));

    let policy = FailurePolicy::default().with_max_retries(1).with_retry_delay(Duration::from_millis(1));

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
async fn retry_policy_retries_sync_handler() {
    let bus = EventBus::new(16);
    let attempts = Arc::new(AtomicUsize::new(0));

    let policy = FailurePolicy::default().with_max_retries(1).with_retry_delay(Duration::from_millis(1));

    let _ = bus
        .subscribe_with_policy(
            FailOnceSync {
                attempts: Arc::clone(&attempts),
            },
            policy,
        )
        .await
        .expect("subscribe");

    bus.publish(Job).await.expect("publish");

    // Sync handlers complete (including retries) before publish returns.
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn retry_multiple_attempts() {
    let bus = EventBus::new(16);
    let attempts = Arc::new(AtomicUsize::new(0));

    // Fail the first 3, succeed on the 4th → need max_retries=3.
    let policy = FailurePolicy::default().with_max_retries(3).with_retry_delay(Duration::from_millis(1));

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
    let bus = EventBus::new(16);
    let attempts = Arc::new(AtomicUsize::new(0));

    let policy = FailurePolicy::default()
        .with_max_retries(1)
        .with_retry_delay(Duration::from_millis(50))
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
