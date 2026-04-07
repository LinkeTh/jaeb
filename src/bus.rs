use std::any::TypeId;

use tokio::sync::{mpsc, oneshot};
use tracing::{error, trace};

use crate::actor::{BusMessage, EventBusActor};
use crate::error::EventBusError;
use crate::handler::{IntoHandler, SyncEventHandler};
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

    /// Subscribe a handler for events of type `E`.
    ///
    /// The dispatch mode (async vs sync) is determined automatically by the
    /// handler trait implementation:
    /// - [`EventHandler<E>`](crate::handler::EventHandler) → async dispatch
    /// - [`SyncEventHandler<E>`](crate::handler::SyncEventHandler) → sync dispatch
    pub async fn subscribe<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        self.subscribe_with_policy(handler, FailurePolicy::default()).await
    }

    /// Subscribe a handler with a custom [`FailurePolicy`].
    ///
    /// See [`subscribe`](Self::subscribe) for dispatch-mode details.
    pub async fn subscribe_with_policy<E, H, M>(&self, handler: H, failure_policy: FailurePolicy) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        trace!("event_bus.subscribe");
        let registered = handler.into_handler();

        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(BusMessage::Subscribe {
                event_type: TypeId::of::<E>(),
                handler: registered.erased,
                mode: registered.mode,
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

    /// Convenience: subscribe a sync dead-letter handler with `dead_letter: false`
    /// to prevent infinite recursion.
    pub async fn subscribe_dead_letters<H>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        H: SyncEventHandler<DeadLetter>,
    {
        let policy = FailurePolicy::default().with_dead_letter(false);
        self.subscribe_with_policy::<DeadLetter, H, crate::handler::SyncMode>(handler, policy)
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
    /// After shutdown, all publish/subscribe operations return
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
}
