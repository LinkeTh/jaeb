// SPDX-License-Identifier: MIT
use jaeb::{EventBus, EventBusError};

// ── Builder validation tests ─────────────────────────────────────────

#[test]
fn build_with_zero_buffer_size_returns_error() {
    let result = EventBus::builder().buffer_size(0).build();
    match result {
        Err(EventBusError::InvalidConfig(msg)) => {
            assert!(msg.contains("buffer_size"), "error message should mention buffer_size: {msg}");
        }
        Err(other) => panic!("expected InvalidConfig error, got {other}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[test]
fn build_with_zero_max_concurrent_async_returns_error() {
    let result = EventBus::builder().max_concurrent_async(0).build();
    match result {
        Err(EventBusError::InvalidConfig(msg)) => {
            assert!(
                msg.contains("max_concurrent_async"),
                "error message should mention max_concurrent_async: {msg}"
            );
        }
        Err(other) => panic!("expected InvalidConfig error, got {other}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[tokio::test]
async fn build_with_valid_config_succeeds() {
    let bus = EventBus::builder()
        .buffer_size(16)
        .max_concurrent_async(4)
        .build()
        .expect("valid config should succeed");
    bus.shutdown().await.expect("shutdown");
}

#[test]
#[should_panic(expected = "buffer size must be greater than zero")]
fn new_with_zero_buffer_panics() {
    let _bus = EventBus::new(0);
}
