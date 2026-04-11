use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::error::EventBusError;
use crate::handler::{IntoHandler, RegisteredHandler, SyncEventHandler};
use crate::middleware::{Middleware, SyncMiddleware, TypedMiddleware, TypedSyncMiddleware};
use crate::registry::{ErasedMiddleware, TypedMiddlewareEntry, TypedMiddlewareSlot};
use crate::subscription::Subscription;
use crate::types::{DeadLetter, Event, IntoSubscriptionPolicy, SubscriptionPolicy, SyncSubscriptionPolicy};

use super::EventBus;

impl EventBus {
    /// Register a handler for event type `E` using the bus's default
    /// subscription policy.
    ///
    /// The dispatch mode (async vs sync) is inferred from the handler type:
    /// - Types implementing [`EventHandler<E>`](crate::EventHandler) are dispatched
    ///   asynchronously — `publish` spawns a task and may return before the handler
    ///   finishes. The event type must be `Clone` because a separate clone is passed
    ///   to each concurrent invocation.
    /// - Types implementing [`SyncEventHandler<E>`](crate::SyncEventHandler) are
    ///   dispatched synchronously — `publish` waits for the handler to return before
    ///   proceeding.
    /// - Plain `async fn(E)` closures/function pointers select async dispatch.
    /// - Plain `fn(&E)` closures/function pointers select sync dispatch.
    ///
    /// Returns a [`Subscription`] handle. The listener remains active until the
    /// subscription is explicitly unsubscribed or the bus is shut down.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        let mut policy = self.inner.default_subscription_policy;
        if registered.is_sync {
            policy.max_retries = 0;
            policy.retry_strategy = None;
        }
        self.subscribe_internal::<E>(registered, policy, false).await
    }

    /// Register a handler for event type `E` with an explicit subscription
    /// policy.
    ///
    /// Behaves the same as [`subscribe`](Self::subscribe) but overrides the
    /// bus-level default [`SubscriptionPolicy`] for this listener.
    ///
    /// The `policy` parameter accepts either a [`SubscriptionPolicy`] (async
    /// handlers only — priority + retry + dead-letter) or a
    /// [`SyncSubscriptionPolicy`] (all handlers — priority + dead-letter only).
    /// Passing a [`SubscriptionPolicy`] for a sync handler is a compile-time
    /// error enforced by [`IntoSubscriptionPolicy`].
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_with_policy<E, H, M>(&self, handler: H, policy: impl IntoSubscriptionPolicy<M>) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        self.subscribe_internal::<E>(registered, policy.into_subscription_policy(), false).await
    }

    async fn subscribe_internal<E: Event>(
        &self,
        registered: RegisteredHandler,
        subscription_policy: SubscriptionPolicy,
        once: bool,
    ) -> Result<Subscription, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        #[cfg(feature = "trace")]
        tracing::trace!("event_bus.subscribe {:?}", &registered.name);
        let id = self.next_subscription_id();
        let listener = (registered.register)(id, subscription_policy, once);

        let mut registry = self.inner.registry.lock().await;
        registry.add_listener(TypeId::of::<E>(), std::any::type_name::<E>(), listener);
        self.refresh_snapshot_locked(&registry).await;

        Ok(Subscription::new(id, registered.name, self.clone()))
    }

    /// Register a sync handler that receives [`DeadLetter`] events.
    ///
    /// Dead-letter listeners must be **synchronous** ([`SyncEventHandler<DeadLetter>`]).
    /// The listener's own `dead_letter` flag is forced to `false` to prevent
    /// infinite recursion (a failure in a dead-letter listener cannot produce
    /// another dead letter).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_dead_letters<H>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        H: SyncEventHandler<DeadLetter>,
    {
        let policy = SyncSubscriptionPolicy::default().with_dead_letter(false);
        self.subscribe_with_policy::<DeadLetter, H, crate::handler::SyncMode>(handler, policy)
            .await
    }

    /// Register a handler that automatically unsubscribes after its first
    /// invocation.
    ///
    /// Uses the bus default subscription policy with `dead_letter` inherited
    /// from that policy. For custom dead-letter behaviour use
    /// [`subscribe_once_with_policy`](Self::subscribe_once_with_policy).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_once<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let policy = SyncSubscriptionPolicy {
            priority: self.inner.default_subscription_policy.priority,
            dead_letter: self.inner.default_subscription_policy.dead_letter,
        };
        self.subscribe_once_with_policy(handler, policy).await
    }

    /// Register a one-shot handler with an explicit [`SyncSubscriptionPolicy`].
    ///
    /// The handler fires at most once and then unsubscribes itself. Retries
    /// are not supported for one-shot listeners, so only
    /// [`SyncSubscriptionPolicy`] is accepted.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_once_with_policy<E, H, M>(
        &self,
        handler: H,
        subscription_policy: SyncSubscriptionPolicy,
    ) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        self.subscribe_internal::<E>(registered, subscription_policy.into(), true).await
    }

    /// Add a global async middleware that intercepts **all** event types.
    ///
    /// Middleware runs before any listener receives an event. If the middleware
    /// returns [`MiddlewareDecision::Reject`](crate::MiddlewareDecision::Reject),
    /// the event is dropped and [`EventBusError::MiddlewareRejected`] is returned
    /// to the caller.
    ///
    /// Global middlewares run in registration order. Returns a [`Subscription`]
    /// that can be used to remove the middleware via [`unsubscribe`](Self::unsubscribe).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn add_middleware<M: Middleware>(&self, middleware: M) -> Result<Subscription, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let erased = ErasedMiddleware::Async(Arc::new(move |event_name: &'static str, event| {
            let mw = Arc::clone(&mw);
            Box::pin(async move { mw.process(event_name, event.as_ref()).await }) as Pin<Box<dyn Future<Output = _> + Send>>
        }));

        let id = self.next_subscription_id();
        let mut registry = self.inner.registry.lock().await;
        registry.add_global_middleware(id, erased);
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(id, None, self.clone()))
    }

    /// Add a global **sync** middleware that intercepts all event types.
    ///
    /// Behaves like [`add_middleware`](Self::add_middleware) but the middleware
    /// function is synchronous and runs inline during dispatch.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn add_sync_middleware<M: SyncMiddleware>(&self, middleware: M) -> Result<Subscription, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let erased = ErasedMiddleware::Sync(Arc::new(move |event_name: &'static str, event: &(dyn std::any::Any + Send + Sync)| {
            mw.process(event_name, event)
        }));

        let id = self.next_subscription_id();
        let mut registry = self.inner.registry.lock().await;
        registry.add_global_middleware(id, erased);
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(id, None, self.clone()))
    }

    /// Add an async middleware scoped to a single event type `E`.
    ///
    /// Unlike [`add_middleware`](Self::add_middleware), typed middleware only
    /// intercepts events of type `E`. Multiple typed middlewares for the same
    /// type run in registration order.
    ///
    /// Returns a [`Subscription`] that can be used to remove the middleware.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn add_typed_middleware<E, M>(&self, middleware: M) -> Result<Subscription, EventBusError>
    where
        E: Event,
        M: TypedMiddleware<E>,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let slot = TypedMiddlewareSlot {
            id: self.next_subscription_id(),
            middleware: TypedMiddlewareEntry::Async(Arc::new(move |event_name, event| {
                let mw = Arc::clone(&mw);
                let event = event.downcast::<E>();
                Box::pin(async move {
                    match event {
                        Ok(event) => mw.process(event_name, event.as_ref()).await,
                        Err(_) => crate::middleware::MiddlewareDecision::Reject("event type mismatch".to_string()),
                    }
                })
            })),
        };

        let mut registry = self.inner.registry.lock().await;
        registry.add_typed_middleware(TypeId::of::<E>(), std::any::type_name::<E>(), slot.clone());
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(slot.id, None, self.clone()))
    }

    /// Add a **sync** middleware scoped to a single event type `E`.
    ///
    /// Behaves like [`add_typed_middleware`](Self::add_typed_middleware) but the
    /// middleware function is synchronous and runs inline during dispatch.
    ///
    /// Returns a [`Subscription`] that can be used to remove the middleware.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn add_typed_sync_middleware<E, M>(&self, middleware: M) -> Result<Subscription, EventBusError>
    where
        E: Event,
        M: TypedSyncMiddleware<E>,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let slot = TypedMiddlewareSlot {
            id: self.next_subscription_id(),
            middleware: TypedMiddlewareEntry::Sync(Arc::new(move |event_name, event| {
                let Some(event) = event.downcast_ref::<E>() else {
                    return crate::middleware::MiddlewareDecision::Reject("event type mismatch".to_string());
                };
                mw.process(event_name, event)
            })),
        };

        let mut registry = self.inner.registry.lock().await;
        registry.add_typed_middleware(TypeId::of::<E>(), std::any::type_name::<E>(), slot.clone());
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(slot.id, None, self.clone()))
    }
}
