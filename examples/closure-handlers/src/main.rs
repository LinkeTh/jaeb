//! Use closures (sync and async) instead of struct-based handlers.
//!
//! Sync closures take `&E`, async closures take `E` by value (a clone).

use jaeb::{EventBus, HandlerResult};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Clicked {
    button: &'static str,
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");

    // Sync closure: receives &Clicked.
    let _ = bus
        .subscribe::<Clicked, _, _>(|event: &Clicked| -> HandlerResult {
            println!("sync closure: {} clicked", event.button);
            Ok(())
        })
        .await
        .expect("subscribe sync closure failed");

    // Async closure: receives Clicked by value (cloned from the published event).
    let _ = bus
        .subscribe::<Clicked, _, _>(|event: Clicked| async move {
            println!("async closure: {} clicked", event.button);
            Ok(())
        })
        .await
        .expect("subscribe async closure failed");

    bus.publish(Clicked { button: "submit" }).await.expect("publish failed");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    bus.shutdown().await.expect("shutdown failed");

    println!("done");
}
