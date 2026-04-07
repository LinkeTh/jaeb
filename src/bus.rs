use std::{any::TypeId, future::Future, sync::Arc};

use tokio::sync::{mpsc, oneshot};
use tracing::{error, trace, warn};

use crate::actor::{BusMessage, ErasedHandler, EventBusActor, ListenerMode};
use crate::error::{EventBusError, HandlerResult};
use crate::handler::EventHandler;
use crate::handler::SyncEventHandler;
use crate::subscription::Subscription;
use crate::types::{DeadLetter, Event, FailurePolicy};

#[derive(Clone)]
pub struct EventBus {
    tx: mpsc::Sender<BusMessage>,
}

impl EventBus {
    pub fn new(buffer: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        let actor = EventBusActor::new(tx.clone(), rx);
        tokio::spawn(actor.run());
        Self { tx }
    }

    pub async fn subscribe_sync<E, F>(&self, callback: F) -> Result<Subscription, EventBusError>
    where
        E: Event,
        F: Fn(&E) -> HandlerResult + Send + Sync + 'static,
    {
        self.subscribe_sync_with_policy(callback, FailurePolicy::default()).await
    }

    pub async fn subscribe_sync_with_policy<E, F>(&self, callback: F, failure_policy: FailurePolicy) -> Result<Subscription, EventBusError>
    where
        E: Event,
        F: Fn(&E) -> HandlerResult + Send + Sync + 'static,
    {
        trace!("event_bus.subscribe_sync");

        let handler: ErasedHandler = Arc::new(move |any: Arc<dyn std::any::Any + Send + Sync>| {
            let result = if let Some(event) = any.as_ref().downcast_ref::<E>() {
                callback(event)
            } else {
                warn!(expected = std::any::type_name::<E>(), "subscribe_sync.downcast_failed");
                Ok(())
            };

            Box::pin(async move { result })
        });

        self.subscribe_erased::<E>(handler, ListenerMode::Sync, failure_policy).await
    }

    pub async fn subscribe_async<E, F, Fut>(&self, callback: F) -> Result<Subscription, EventBusError>
    where
        E: Event + Clone,
        F: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static,
    {
        self.subscribe_async_with_policy(callback, FailurePolicy::default()).await
    }

    pub async fn subscribe_async_with_policy<E, F, Fut>(&self, callback: F, failure_policy: FailurePolicy) -> Result<Subscription, EventBusError>
    where
        E: Event + Clone,
        F: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static,
    {
        trace!("event_bus.subscribe_async");

        let callback = Arc::new(callback);
        let handler: ErasedHandler = Arc::new(move |any: Arc<dyn std::any::Any + Send + Sync>| {
            let callback = Arc::clone(&callback);
            if let Some(event) = any.as_ref().downcast_ref::<E>() {
                let event = event.clone();
                Box::pin(async move { callback(event).await })
            } else {
                warn!(expected = std::any::type_name::<E>(), "subscribe_async.downcast_failed");
                Box::pin(async { Ok(()) })
            }
        });

        self.subscribe_erased::<E>(handler, ListenerMode::Async, failure_policy).await
    }

    pub async fn subscribe_dead_letters<F>(&self, callback: F) -> Result<Subscription, EventBusError>
    where
        F: Fn(&DeadLetter) -> HandlerResult + Send + Sync + 'static,
    {
        let policy = FailurePolicy::default().with_dead_letter(false);
        self.subscribe_sync_with_policy::<DeadLetter, _>(callback, policy).await
    }

    pub async fn register<E, H>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event + Clone,
        H: EventHandler<E>,
    {
        self.register_with_policy(handler, FailurePolicy::default()).await
    }

    pub async fn register_with_policy<E, H>(&self, handler: H, failure_policy: FailurePolicy) -> Result<Subscription, EventBusError>
    where
        E: Event + Clone,
        H: EventHandler<E>,
    {
        let handler = Arc::new(handler);
        self.subscribe_async_with_policy(
            move |event: E| {
                let handler = Arc::clone(&handler);
                async move { handler.handle(&event).await }
            },
            failure_policy,
        )
        .await
    }

    pub async fn register_sync<E, H>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: SyncEventHandler<E>,
    {
        self.register_sync_with_policy(handler, FailurePolicy::default()).await
    }

    pub async fn register_sync_with_policy<E, H>(&self, handler: H, failure_policy: FailurePolicy) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: SyncEventHandler<E>,
    {
        let handler = Arc::new(handler);
        self.subscribe_sync_with_policy(move |event: &E| handler.handle(event), failure_policy)
            .await
    }

    /// Publish an event and wait until the actor has dispatched it.
    ///
    /// This waits for synchronous listeners to complete, but may return before
    /// asynchronous listeners finish.
    pub async fn publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        trace!("event_bus.publish");

        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(BusMessage::Publish {
                event_type: TypeId::of::<E>(),
                event: Box::new(event),
                event_name: std::any::type_name::<E>(),
                ack: Some(ack_tx),
            })
            .await
            .map_err(|e| {
                error!(operation = "publish", error = %e, "event_bus.send_failed");
                EventBusError::ActorStopped
            })?;

        ack_rx.await.map_err(|_| {
            error!(operation = "publish", "event_bus.ack_wait_failed");
            EventBusError::ActorStopped
        })
    }

    /// Try to publish without waiting or blocking.
    ///
    /// Returns [`EventBusError::ChannelFull`] when the internal queue is full.
    pub fn try_publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        trace!("event_bus.try_publish");

        match self.tx.try_send(BusMessage::Publish {
            event_type: TypeId::of::<E>(),
            event: Box::new(event),
            event_name: std::any::type_name::<E>(),
            ack: None,
        }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(EventBusError::ChannelFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(EventBusError::ActorStopped),
        }
    }

    pub async fn unsubscribe(&self, subscription_id: crate::types::SubscriptionId) -> Result<bool, EventBusError> {
        trace!(subscription_id = subscription_id.as_u64(), "event_bus.unsubscribe");

        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(BusMessage::Unsubscribe {
                subscription_id,
                ack: ack_tx,
            })
            .await
            .map_err(|e| {
                error!(operation = "unsubscribe", error = %e, "event_bus.send_failed");
                EventBusError::ActorStopped
            })?;

        ack_rx.await.map_err(|_| {
            error!(operation = "unsubscribe", "event_bus.ack_wait_failed");
            EventBusError::ActorStopped
        })
    }

    /// Gracefully stop the actor and wait for queued publish messages plus
    /// in-flight async handlers.
    ///
    /// After shutdown, all publish/subscribe/register operations return
    /// [`EventBusError::ActorStopped`].
    pub async fn shutdown(&self) -> Result<(), EventBusError> {
        trace!("event_bus.shutdown");

        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx.send(BusMessage::Shutdown { ack: ack_tx }).await.map_err(|e| {
            error!(operation = "shutdown", error = %e, "event_bus.send_failed");
            EventBusError::ActorStopped
        })?;

        ack_rx.await.map_err(|_| {
            error!(operation = "shutdown", "event_bus.ack_wait_failed");
            EventBusError::ActorStopped
        })
    }

    async fn subscribe_erased<E>(
        &self,
        handler: ErasedHandler,
        mode: ListenerMode,
        failure_policy: FailurePolicy,
    ) -> Result<Subscription, EventBusError>
    where
        E: Event,
    {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(BusMessage::Subscribe {
                event_type: TypeId::of::<E>(),
                handler,
                mode,
                failure_policy,
                ack: ack_tx,
            })
            .await
            .map_err(|e| {
                error!(operation = "subscribe", error = %e, "event_bus.send_failed");
                EventBusError::ActorStopped
            })?;

        let subscription_id = ack_rx.await.map_err(|_| {
            error!(operation = "subscribe", "event_bus.ack_wait_failed");
            EventBusError::ActorStopped
        })?;

        Ok(Subscription::new(subscription_id, self.clone()))
    }
}
