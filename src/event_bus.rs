use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, trace, warn};

#[cfg(feature = "metrics")]
use metrics::{counter, histogram};
#[cfg(feature = "metrics")]
pub(crate) struct TimerGuard {
    start: std::time::Instant,
    name: &'static str,
    event: &'static str,
}

#[cfg(feature = "metrics")]
impl TimerGuard {
    pub fn start(name: &'static str, event: &'static str) -> Self {
        Self {
            start: std::time::Instant::now(),
            name,
            event,
        }
    }
}

#[cfg(feature = "metrics")]
impl Drop for TimerGuard {
    fn drop(&mut self) {
        let dur = self.start.elapsed();
        // Record in seconds as f64 for histogram
        let histogram = histogram!(self.name, "event" => self.event);
        histogram.record(dur.as_secs_f64());
    }
}

pub trait Event: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Event for T {}

// A listener erased to take an Arc-erased event and return an async future.
type HandlerFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

type ArcEvent = Arc<dyn Any + Send + Sync>;
type ErasedHandler = Arc<dyn Fn(ArcEvent) -> HandlerFuture + Send + Sync + 'static>;

#[derive(Clone)]
struct Listener {
    handler: ErasedHandler,
    is_async: bool,
}

enum BusMessage {
    Subscribe {
        event_type: TypeId,
        handler: ErasedHandler,
        is_async: bool,
        ack: oneshot::Sender<()>,
    },
    Publish {
        event_type: TypeId,
        event: Box<dyn Any + Send + Sync>,
        event_name: &'static str,
        ack: oneshot::Sender<()>,
    },
}

struct EventBusActor {
    rx: mpsc::Receiver<BusMessage>,
    listeners: HashMap<TypeId, Vec<Listener>>, // keyed by event TypeId
}

impl EventBusActor {
    fn new(rx: mpsc::Receiver<BusMessage>) -> Self {
        Self {
            rx,
            listeners: HashMap::new(),
        }
    }

    async fn run(mut self) {
        trace!("event_bus_actor.run");
        while let Some(msg) = self.rx.recv().await {
            match msg {
                BusMessage::Subscribe {
                    event_type,
                    handler,
                    is_async,
                    ack,
                } => {
                    let list = self.listeners.entry(event_type).or_default();
                    list.push(Listener { handler, is_async });
                    if let Err(_e) = ack.send(()) {
                        // Sender side already applied the subscription; the receiver went away.
                        // Nothing actionable, but surface it for visibility.
                        warn!("subscribe.ack_receiver_dropped");
                    }
                }
                BusMessage::Publish {
                    event_type,
                    event,
                    event_name,
                    ack,
                } => {
                    let listeners = self.listeners.get(&event_type).cloned().unwrap_or_default();
                    let event: ArcEvent = Arc::from(event);

                    #[cfg(feature = "metrics")]
                    counter!("eventbus.publish", "event" => event_name).increment(1);

                    let listeners_count = listeners.len();
                    debug!(event = event_name, listeners = listeners_count, "publish.dispatch");

                    let mut tasks = Vec::with_capacity(listeners_count);
                    for listener in listeners {
                        let event = event.clone();

                        if listener.is_async {
                            debug!(event = event_name, "publish.async");
                            tokio::spawn(async move {
                                #[cfg(feature = "metrics")]
                                let _timer = TimerGuard::start("eventbus.handler.duration", event_name);

                                let handler = listener.handler;
                                handler(event).await;
                            });
                        } else {
                            debug!(event = event_name, "publish.sync");

                            #[cfg(feature = "metrics")]
                            let _timer = TimerGuard::start("eventbus.handler.duration", event_name);

                            let handler = listener.handler;
                            tasks.push(tokio::spawn(async move { handler(event).await }));
                        }
                    }
                    for task in tasks {
                        match task.await {
                            Ok(_) => {}
                            Err(e) => {
                                error!(event = event_name, error = %e, "handler.join_error");

                                #[cfg(feature = "metrics")]
                                counter!("eventbus.handler.join_error", "event" => event_name).increment(1);
                            }
                        }
                    }
                    if let Err(_e) = ack.send(()) {
                        // Caller may have been dropped/cancelled. Log for observability.
                        warn!(event = event_name, "publish.ack_receiver_dropped");
                    }
                }
            }
        }
        debug!("event_bus_actor.stopped");
    }
}

#[derive(Clone)]
pub struct EventBus {
    tx: mpsc::Sender<BusMessage>,
}

impl EventBus {
    pub fn new(buffer: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        let actor = EventBusActor::new(rx);
        tokio::spawn(actor.run());
        Self { tx }
    }

    // Register a synchronous listener: `Fn(&E)`
    pub async fn subscribe_sync<E, F>(&self, callback: F)
    where
        E: Event,
        F: Fn(&E) + Send + Sync + 'static,
    {
        trace!("event_bus.subscribe_sync");
        let handler: ErasedHandler = Arc::new(move |any: ArcEvent| {
            if let Some(event) = any.as_ref().downcast_ref::<E>() {
                callback(event);
            }
            Box::pin(async {})
        });
        let (ack_tx, ack_rx) = oneshot::channel();
        match self
            .tx
            .send(BusMessage::Subscribe {
                event_type: TypeId::of::<E>(),
                handler,
                is_async: false,
                ack: ack_tx,
            })
            .await
        {
            Ok(()) => {}
            Err(e) => {
                error!(error = %e, "subscribe_sync.send_failed");
                return;
            }
        }
        if let Err(_e) = ack_rx.await {
            error!("subscribe_sync.ack_wait_failed: actor not available");
        }
    }

    // Register an async listener: `Fn(E) -> impl Future<Output=()>`
    pub async fn subscribe_async<E, F, Fut>(&self, callback: F)
    where
        E: Event + Clone,
        F: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        trace!("event_bus.subscribe_async");
        let callback = Arc::new(callback);
        let handler: ErasedHandler = Arc::new(move |any: ArcEvent| {
            let callback = callback.clone();
            if let Some(event) = any.as_ref().downcast_ref::<E>() {
                let event = event.clone();
                Box::pin(async move { callback(event).await })
            } else {
                Box::pin(async {})
            }
        });
        let (ack_tx, ack_rx) = oneshot::channel();
        match self
            .tx
            .send(BusMessage::Subscribe {
                event_type: TypeId::of::<E>(),
                handler,
                is_async: true,
                ack: ack_tx,
            })
            .await
        {
            Ok(()) => {}
            Err(e) => {
                error!(error = %e, "subscribe_async.send_failed");
                return;
            }
        }
        if let Err(_e) = ack_rx.await {
            error!("subscribe_async.ack_wait_failed: actor not available");
        }
    }

    /// Publish an event to all listeners.
    ///
    /// This waits for synchronous listeners to complete, but may return before
    /// asynchronously-registered listeners finish their work.
    pub async fn publish<E>(&self, event: E)
    where
        E: Event + Clone,
    {
        trace!("event_bus.publish");
        let (ack_tx, ack_rx) = oneshot::channel();
        match self
            .tx
            .send(BusMessage::Publish {
                event_type: TypeId::of::<E>(),
                event: Box::new(event),
                event_name: std::any::type_name::<E>(),
                ack: ack_tx,
            })
            .await
        {
            Ok(()) => {}
            Err(e) => {
                error!(error = %e, "publish.send_failed");
                return;
            }
        }
        if let Err(_e) = ack_rx.await {
            error!("publish.ack_wait_failed: actor not available");
        }
    }
}
