use std::sync::Arc;
use std::sync::Mutex;

use jaeb::{AsyncSubscriptionPolicy, EventBus, HandlerResult, SubscriptionDefaults, SyncEventHandler, SyncSubscriptionPolicy};

#[derive(Clone)]
struct PriorityEvent;

struct OrderedSync {
    id: usize,
    order: Arc<Mutex<Vec<usize>>>,
}

impl SyncEventHandler<PriorityEvent> for OrderedSync {
    fn handle(&self, _event: &PriorityEvent, _bus: &EventBus) -> HandlerResult {
        let mut guard = self.order.lock().expect("lock order");
        guard.push(self.id);
        Ok(())
    }
}

#[tokio::test]
async fn sync_priority_orders_high_to_low() {
    let bus = EventBus::builder().build().await.expect("valid config");
    let order = Arc::new(Mutex::new(Vec::new()));

    let low = SyncSubscriptionPolicy::default().with_priority(-10);
    let mid = SyncSubscriptionPolicy::default().with_priority(0);
    let high = SyncSubscriptionPolicy::default().with_priority(10);

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedSync {
                id: 1,
                order: Arc::clone(&order),
            },
            low,
        )
        .await
        .expect("subscribe low");

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedSync {
                id: 2,
                order: Arc::clone(&order),
            },
            high,
        )
        .await
        .expect("subscribe high");

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedSync {
                id: 3,
                order: Arc::clone(&order),
            },
            mid,
        )
        .await
        .expect("subscribe mid");

    bus.publish(PriorityEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let got = order.lock().expect("lock order").clone();
    assert_eq!(got, vec![2, 3, 1]);
}

#[tokio::test]
async fn sync_equal_priority_keeps_fifo_order() {
    let bus = EventBus::builder().build().await.expect("valid config");
    let order = Arc::new(Mutex::new(Vec::new()));

    let p = SyncSubscriptionPolicy::default().with_priority(7);

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedSync {
                id: 1,
                order: Arc::clone(&order),
            },
            p,
        )
        .await
        .expect("subscribe 1");

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedSync {
                id: 2,
                order: Arc::clone(&order),
            },
            p,
        )
        .await
        .expect("subscribe 2");

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedSync {
                id: 3,
                order: Arc::clone(&order),
            },
            p,
        )
        .await
        .expect("subscribe 3");

    bus.publish(PriorityEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let got = order.lock().expect("lock order").clone();
    assert_eq!(got, vec![1, 2, 3]);
}

#[tokio::test]
async fn subscribe_uses_sync_defaults_for_sync_handlers() {
    let bus = EventBus::builder()
        .default_subscription_policies(SubscriptionDefaults {
            policy: AsyncSubscriptionPolicy::default(),
            sync_policy: SyncSubscriptionPolicy::default().with_priority(10),
        })
        .build()
        .await
        .expect("valid config");
    let order = Arc::new(Mutex::new(Vec::new()));

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedSync {
                id: 1,
                order: Arc::clone(&order),
            },
            SyncSubscriptionPolicy::default().with_priority(-10),
        )
        .await
        .expect("subscribe 1");

    let _ = bus
        .subscribe::<PriorityEvent, _, _>(OrderedSync {
            id: 2,
            order: Arc::clone(&order),
        })
        .await
        .expect("subscribe 2");

    bus.publish(PriorityEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    let got = order.lock().expect("lock order").clone();
    assert_eq!(got, vec![2, 1]);
}
