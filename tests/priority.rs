use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, EventHandler, HandlerResult, SubscriptionPolicy, SyncEventHandler, SyncSubscriptionPolicy};

#[derive(Clone)]
struct PriorityEvent;

struct OrderedSync {
    id: usize,
    order: Arc<Mutex<Vec<usize>>>,
}

impl SyncEventHandler<PriorityEvent> for OrderedSync {
    fn handle(&self, _event: &PriorityEvent) -> HandlerResult {
        let mut guard = self.order.lock().expect("lock order");
        guard.push(self.id);
        Ok(())
    }
}

struct OrderedAsync {
    id: usize,
    order: Arc<Mutex<Vec<usize>>>,
    hits: Arc<AtomicUsize>,
}

impl EventHandler<PriorityEvent> for OrderedAsync {
    async fn handle(&self, _event: &PriorityEvent) -> HandlerResult {
        self.hits.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.order.lock().expect("lock order");
        guard.push(self.id);
        Ok(())
    }
}

#[tokio::test]
async fn sync_priority_orders_high_to_low() {
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
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
    let bus = EventBus::builder().buffer_size(64).build().await.expect("valid config");
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
async fn async_priority_orders_spawn_sequence() {
    let bus = EventBus::builder()
        .buffer_size(64)
        .max_concurrent_async(1)
        .build()
        .await
        .expect("valid config");

    let order = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedAsync {
                id: 1,
                order: Arc::clone(&order),
                hits: Arc::clone(&hits),
            },
            SubscriptionPolicy::default().with_priority(-1),
        )
        .await
        .expect("subscribe low");

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedAsync {
                id: 2,
                order: Arc::clone(&order),
                hits: Arc::clone(&hits),
            },
            SubscriptionPolicy::default().with_priority(20),
        )
        .await
        .expect("subscribe high");

    let _ = bus
        .subscribe_with_policy::<PriorityEvent, _, _>(
            OrderedAsync {
                id: 3,
                order: Arc::clone(&order),
                hits: Arc::clone(&hits),
            },
            SubscriptionPolicy::default().with_priority(0),
        )
        .await
        .expect("subscribe mid");

    bus.publish(PriorityEvent).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(hits.load(Ordering::SeqCst), 3);
    let got = order.lock().expect("lock order").clone();
    assert_eq!(got, vec![2, 3, 1]);
}
