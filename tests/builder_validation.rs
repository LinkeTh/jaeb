use jaeb::{EventBus, EventBusError};

// ── Builder validation tests ─────────────────────────────────────────

#[tokio::test]
async fn build_with_zero_buffer_size_returns_error() {
    let result = EventBus::builder().buffer_size(0).build().await;
    match result {
        Err(EventBusError::InvalidConfig(jaeb::ConfigError::ZeroBufferSize)) => {}
        Err(other) => panic!("expected InvalidConfig error, got {other}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[tokio::test]
async fn build_with_zero_max_concurrent_async_returns_error() {
    let result = EventBus::builder().max_concurrent_async(0).build().await;
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
        .await
        .expect("valid config should succeed");
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn build_default_config_succeeds() {
    let bus = EventBus::builder().build().await.expect("default config should succeed");
    bus.shutdown().await.expect("shutdown");
}
