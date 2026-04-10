//! Tests for the `test-utils` feature-gated module.

#![cfg(feature = "test-utils")]

use jaeb::TestBus;

#[derive(Clone, Debug, PartialEq)]
struct Ping(u32);

#[derive(Clone, Debug, PartialEq)]
struct Pong(String);

#[tokio::test]
async fn test_bus_captures_events() {
    let bus = TestBus::new().expect("create TestBus");

    bus.capture::<Ping>().await.expect("capture Ping");

    bus.inner().publish(Ping(1)).await.expect("publish 1");
    bus.inner().publish(Ping(2)).await.expect("publish 2");
    bus.inner().publish(Ping(3)).await.expect("publish 3");

    let captured = bus.published::<Ping>();
    assert_eq!(captured, vec![Ping(1), Ping(2), Ping(3)]);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_bus_multiple_types() {
    let bus = TestBus::new().expect("create TestBus");

    bus.capture::<Ping>().await.expect("capture Ping");
    bus.capture::<Pong>().await.expect("capture Pong");

    bus.inner().publish(Ping(42)).await.expect("publish Ping");
    bus.inner().publish(Pong("hello".into())).await.expect("publish Pong");
    bus.inner().publish(Ping(99)).await.expect("publish Ping 2");

    assert_eq!(bus.published::<Ping>(), vec![Ping(42), Ping(99)]);
    assert_eq!(bus.published::<Pong>(), vec![Pong("hello".into())]);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_bus_empty_before_publish() {
    let bus = TestBus::new().expect("create TestBus");

    bus.capture::<Ping>().await.expect("capture Ping");

    let captured = bus.published::<Ping>();
    assert!(captured.is_empty(), "should be empty before any publish");

    // Also check for a type we never captured.
    let uncaptured = bus.published::<Pong>();
    assert!(uncaptured.is_empty(), "uncaptured type should return empty vec");

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_bus_assert_count() {
    let bus = TestBus::new().expect("create TestBus");

    bus.capture::<Ping>().await.expect("capture Ping");

    bus.inner().publish(Ping(1)).await.expect("publish");
    bus.inner().publish(Ping(2)).await.expect("publish");

    bus.assert_count::<Ping>(2);
    bus.assert_empty::<Pong>();

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
#[should_panic(expected = "expected 5 events, got 2")]
async fn test_bus_assert_count_panics_on_mismatch() {
    let bus = TestBus::new().expect("create TestBus");

    bus.capture::<Ping>().await.expect("capture Ping");

    bus.inner().publish(Ping(1)).await.expect("publish");
    bus.inner().publish(Ping(2)).await.expect("publish");

    bus.assert_count::<Ping>(5); // should panic
}

#[tokio::test]
async fn test_bus_capture_idempotent() {
    let bus = TestBus::new().expect("create TestBus");

    bus.capture::<Ping>().await.expect("capture 1");
    bus.capture::<Ping>().await.expect("capture 2 (idempotent)");

    bus.inner().publish(Ping(1)).await.expect("publish");

    // Should have exactly 1 event, not 2 (from duplicate capture).
    bus.assert_count::<Ping>(1);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_bus_builder() {
    let bus = TestBus::builder().buffer_size(32).build().expect("build TestBus");

    bus.capture::<Ping>().await.expect("capture");
    bus.inner().publish(Ping(7)).await.expect("publish");
    bus.assert_count::<Ping>(1);

    bus.shutdown().await.expect("shutdown");
}
