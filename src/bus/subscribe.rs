use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::EventBusError;
use crate::handler::{HandlerPolicy, IntoHandler, RegisteredHandler, SyncEventHandler, into_listener_policy};
use crate::middleware::{Middleware, SyncMiddleware, TypedMiddleware, TypedSyncMiddleware};
use crate::registry::{ErasedMiddleware, EventType, ListenerPolicy, TypedMiddlewareEntry, TypedMiddlewareSlot};
use crate::subscription::Subscription;
use crate::types::{DeadLetter, Event, SyncSubscriptionPolicy};

use super::EventBus;

impl EventBus {
    /// Register a handler for event type `E` using the bus's default
    /// subscription policy.
    ///
    /// The dispatch mode (async vs sync) is inferred from the handler type:
    /// - Types implementing [`EventHandler<E>`](crate::EventHandler) are dispatched
    ///   asynchronously — `publish` spawns a task and may return before the handler
    ///   finishes. The event is `Arc`-wrapped internally; no `Clone` bound is required.
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
        H::Policy: HandlerPolicy,
    {
        self.subscribe_internal::<E, H::Policy>(handler.into_handler(), H::default_policy(&self.inner.subscription_defaults), false)
            .await
    }

    /// Register a handler for event type `E` with an explicit subscription
    /// policy.
    ///
    /// Behaves the same as [`subscribe`](Self::subscribe) but overrides the
    /// bus-level default subscription policy for this listener.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_with_policy<E, H, M>(&self, handler: H, policy: H::Policy) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
        H::Policy: HandlerPolicy,
    {
        self.subscribe_internal::<E, H::Policy>(handler.into_handler(), policy, false).await
    }

    async fn subscribe_internal<E: Event, P>(
        &self,
        registered: RegisteredHandler,
        subscription_policy: P,
        once: bool,
    ) -> Result<Subscription, EventBusError>
    where
        P: HandlerPolicy,
    {
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }

        #[cfg(feature = "trace")]
        tracing::trace!("event_bus.subscribe {:?}", &registered.name());
        let id = self.next_subscription_id();
        let subscription_policy = into_listener_policy(subscription_policy);
        let (mut listener, name) = match (registered, subscription_policy) {
            (RegisteredHandler::Async { register, name }, ListenerPolicy::Async(policy)) => (register(id, policy, once, self.clone()), name),
            (RegisteredHandler::Sync { register, name }, ListenerPolicy::Sync(policy)) => (register(id, policy, once, self.clone()), name),
            (RegisteredHandler::Async { .. }, ListenerPolicy::Sync(_)) => {
                panic!("async handler registered with sync policy")
            }
            (RegisteredHandler::Sync { .. }, ListenerPolicy::Async(_)) => {
                panic!("sync handler registered with async policy")
            }
        };
        listener.registration_order = id.as_u64();

        let mut registry = self.inner.registry.lock().await;
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        registry.add_listener(TypeId::of::<E>(), std::any::type_name::<E>(), listener);
        self.refresh_snapshot_locked(&registry).await;

        Ok(Subscription::new(id, name, self.clone()))
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
        self.subscribe_internal::<DeadLetter, SyncSubscriptionPolicy>(handler.into_handler(), policy, false)
            .await
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
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let erased = ErasedMiddleware::Async(Arc::new(move |event_name: &'static str, event: EventType| {
            let mw = Arc::clone(&mw);
            Box::pin(async move { mw.process(event_name, event.as_ref()).await }) as Pin<Box<dyn Future<Output = _> + Send>>
        }));

        let id = self.next_subscription_id();
        let mut registry = self.inner.registry.lock().await;
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
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
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let erased = ErasedMiddleware::Sync(Arc::new(move |event_name: &'static str, event: &(dyn std::any::Any + Send + Sync)| {
            mw.process(event_name, event)
        }));

        let id = self.next_subscription_id();
        let mut registry = self.inner.registry.lock().await;
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
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
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let slot = TypedMiddlewareSlot {
            id: self.next_subscription_id(),
            middleware: TypedMiddlewareEntry::Async(Arc::new(move |event_name, event: EventType| {
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
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
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
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let slot = TypedMiddlewareSlot {
            id: self.next_subscription_id(),
            middleware: TypedMiddlewareEntry::Sync(Arc::new(move |event_name, event: &(dyn std::any::Any + Send + Sync)| {
                let Some(event) = event.downcast_ref::<E>() else {
                    return crate::middleware::MiddlewareDecision::Reject("event type mismatch".to_string());
                };
                mw.process(event_name, event)
            })),
        };

        let mut registry = self.inner.registry.lock().await;
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        registry.add_typed_middleware(TypeId::of::<E>(), std::any::type_name::<E>(), slot.clone());
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(slot.id, None, self.clone()))
    }
}
