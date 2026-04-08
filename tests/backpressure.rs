// SPDX-License-Identifier: MIT
use jaeb::{EventBus, EventBusError};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Msg;

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn try_publish_reports_channel_full() {
    let bus = EventBus::new(1);

    bus.try_publish(Msg).expect("first try_publish should succeed");

    let err = bus.try_publish(Msg).expect_err("second try_publish should fail");
    assert_eq!(err, EventBusError::ChannelFull);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn try_publish_after_shutdown_returns_actor_stopped() {
    let bus = EventBus::new(16);
    bus.shutdown().await.expect("shutdown");

    let err = bus.try_publish(Msg).expect_err("try_publish after shutdown");
    assert_eq!(err, EventBusError::ActorStopped);
}
