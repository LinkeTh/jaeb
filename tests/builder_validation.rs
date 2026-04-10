use jaeb::{EventBus, EventBusError};

// ── Builder validation tests ─────────────────────────────────────────

#[test]
fn build_with_zero_buffer_size_returns_error() {
    let result = EventBus::builder().buffer_size(0).build();
    match result {
        Err(EventBusError::InvalidConfig(jaeb::ConfigError::ZeroBufferSize)) => {}
        Err(other) => panic!("expected InvalidConfig error, got {other}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[test]
fn build_with_zero_max_concurrent_async_returns_error() {
    let result = EventBus::builder().max_concurrent_async(0).build();
    match result {
        Err(EventBusError::InvalidConfig(jaeb::ConfigError::ZeroConcurrency)) => {}
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
fn new_with_zero_buffer_returns_error() {
    let result = EventBus::new(0);
    match result {
        Err(EventBusError::InvalidConfig(jaeb::ConfigError::ZeroBufferSize)) => {}
        Err(other) => panic!("expected InvalidConfig(ZeroBufferSize) error, got {other}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}
