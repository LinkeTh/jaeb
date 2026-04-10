//! Retry strategies: Fixed, Exponential, and ExponentialWithJitter.
//!
//! Each handler fails a few times before succeeding, demonstrating the
//! configured retry behaviour.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, EventHandler, FailurePolicy, HandlerResult, RetryStrategy};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Job;

// ── Handlers ────────────────────────────────────────────────────────────

struct FlakyHandler {
    label: &'static str,
    attempts: Arc<AtomicUsize>,
    fail_count: usize,
}

impl EventHandler<Job> for FlakyHandler {
    async fn handle(&self, _event: &Job) -> HandlerResult {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.fail_count {
            println!("[{}] attempt {attempt}: failing", self.label);
            return Err(format!("{} transient failure", self.label).into());
        }
        println!("[{}] attempt {attempt}: success", self.label);
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");

    // Fixed: wait 50ms between each retry.
    let fixed_policy = FailurePolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(50)))
        .with_dead_letter(false);
    let _ = bus
        .subscribe_with_policy::<Job, _, _>(
            FlakyHandler {
                label: "fixed",
                attempts: Arc::new(AtomicUsize::new(0)),
                fail_count: 1,
            },
            fixed_policy,
        )
        .await
        .expect("subscribe failed");

    // Exponential: 25ms, 50ms, 100ms, ...
    let exp_policy = FailurePolicy::default()
        .with_max_retries(3)
        .with_retry_strategy(RetryStrategy::Exponential {
            base: Duration::from_millis(25),
            max: Duration::from_millis(200),
        })
        .with_dead_letter(false);
    let _ = bus
        .subscribe_with_policy::<Job, _, _>(
            FlakyHandler {
                label: "exponential",
                attempts: Arc::new(AtomicUsize::new(0)),
                fail_count: 2,
            },
            exp_policy,
        )
        .await
        .expect("subscribe failed");

    // ExponentialWithJitter: randomised delay up to the exponential cap.
    let jitter_policy = FailurePolicy::default()
        .with_max_retries(3)
        .with_retry_strategy(RetryStrategy::ExponentialWithJitter {
            base: Duration::from_millis(25),
            max: Duration::from_millis(200),
        })
        .with_dead_letter(false);
    let _ = bus
        .subscribe_with_policy::<Job, _, _>(
            FlakyHandler {
                label: "jitter",
                attempts: Arc::new(AtomicUsize::new(0)),
                fail_count: 2,
            },
            jitter_policy,
        )
        .await
        .expect("subscribe failed");

    bus.publish(Job).await.expect("publish failed");

    // Let retries complete, then shut down.
    tokio::time::sleep(Duration::from_secs(1)).await;
    bus.shutdown().await.expect("shutdown failed");

    println!("done");
}
