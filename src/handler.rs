use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::deps::Deps;
use crate::error::{EventBusError, HandlerResult};
use crate::registry::{ErasedAsyncHandlerFn, ErasedSyncHandlerFn, EventType, ListenerEntry, ListenerKind};
use crate::subscription::Subscription;
use crate::types::Event;

/// Trait for **asynchronous** event handlers.
///
/// Implement this trait on a struct to receive events of type `E`
/// asynchronously. When an event is published:
/// - The event is cloned once per registered async listener (hence the
///   `E: Clone` bound).
/// - Each invocation is spawned as an independent Tokio task.
/// - [`EventBus::publish`](crate::EventBus::publish) returns once all async
///   handler tasks have been *spawned*, not necessarily *completed*.
///
/// # Examples
///
/// ```
/// use jaeb::{EventHandler, HandlerResult};
///
/// #[derive(Clone)]
/// struct OrderPlaced { id: u64 }
///
/// struct EmailNotifier;
///
/// impl EventHandler<OrderPlaced> for EmailNotifier {
///     async fn handle(&self, event: &OrderPlaced) -> HandlerResult {
///         // send email …
///         Ok(())
///     }
/// }
/// ```
pub trait EventHandler<E: Event + Clone>: Send + Sync + 'static {
    /// Handle a single published event.
    ///
    /// Called with a shared reference to the cloned event. Return `Ok(())`
    /// on success or a [`HandlerError`](crate::HandlerError) on failure.
    /// Failures are subject to the listener's
    /// [`SubscriptionPolicy`](crate::SubscriptionPolicy).
    fn handle(&self, event: &E) -> impl Future<Output = HandlerResult> + Send;

    /// Return an optional human-readable name for this listener.
    ///
    /// The name appears in [`HandlerInfo`](crate::HandlerInfo) (reported by
    /// [`EventBus::stats`](crate::EventBus::stats)) and in
    /// [`DeadLetter::handler_name`](crate::DeadLetter::handler_name).
    ///
    /// Defaults to `None`; the standalone `#[handler]` macro (feature
    /// `macros`) sets this automatically from the function name.
    fn name(&self) -> Option<&'static str> {
        None
    }
}

/// Trait for **synchronous** event handlers.
///
/// Implement this trait on a struct to receive events of type `E`
/// synchronously. When an event is published:
/// - The handler is called inline in a serialized per-event-type FIFO lane.
/// - [`EventBus::publish`](crate::EventBus::publish) waits for the handler to
///   return before proceeding.
/// - Sync handlers do **not** require `E: Clone` because the original event
///   reference is passed directly.
///
/// Sync handlers must use
/// [`SyncSubscriptionPolicy`](crate::SyncSubscriptionPolicy); passing a
/// [`SubscriptionPolicy`](crate::SubscriptionPolicy) for a sync handler is a
/// compile-time error.
///
/// # Examples
///
/// ```
/// use jaeb::{SyncEventHandler, HandlerResult};
///
/// #[derive(Clone)]
/// struct UserDeleted { id: u64 }
///
/// struct AuditLog;
///
/// impl SyncEventHandler<UserDeleted> for AuditLog {
///     fn handle(&self, event: &UserDeleted) -> HandlerResult {
///         // write to audit log …
///         Ok(())
///     }
/// }
/// ```
pub trait SyncEventHandler<E: Event>: Send + Sync + 'static {
    /// Handle a single published event synchronously.
    ///
    /// Called with a shared reference to the event. Return `Ok(())` on success
    /// or a [`HandlerError`](crate::HandlerError) on failure. On failure a
    /// [`DeadLetter`](crate::DeadLetter) is emitted if the listener's policy
    /// has `dead_letter` enabled.
    fn handle(&self, event: &E) -> HandlerResult;

    /// Return an optional human-readable name for this listener.
    ///
    /// See [`EventHandler::name`] for details.
    fn name(&self) -> Option<&'static str> {
        None
    }
}

/// Marker type that selects **async struct** dispatch for [`IntoHandler`].
///
/// Used as the `Mode` type parameter when a struct implementing
/// [`EventHandler<E>`] is passed to a `subscribe_*` method.
pub struct AsyncMode;
/// Marker type that selects **sync struct** dispatch for [`IntoHandler`].
///
/// Used as the `Mode` type parameter when a struct implementing
/// [`SyncEventHandler<E>`] is passed to a `subscribe_*` method.
pub struct SyncMode;
/// Marker type that selects **async function** dispatch for [`IntoHandler`].
///
/// Used as the `Mode` type parameter when a plain `async fn(E) -> HandlerResult`
/// function pointer or closure is passed to a `subscribe_*` method.
pub struct AsyncFnMode;
/// Marker type that selects **sync function** dispatch for [`IntoHandler`].
///
/// Used as the `Mode` type parameter when a plain `fn(&E) -> HandlerResult`
/// function pointer or closure is passed to a `subscribe_*` method.
pub struct SyncFnMode;

pub(crate) type RegisterFn = Box<dyn FnOnce(crate::types::SubscriptionId, crate::types::SubscriptionPolicy, bool) -> ListenerEntry + Send>;

pub(crate) struct RegisteredHandler {
    pub register: RegisterFn,
    pub name: Option<&'static str>,
    pub is_sync: bool,
}

/// Type-erases a concrete handler into the internal representation
/// expected by the bus registry.
///
/// This trait is implemented for:
/// - Structs implementing [`EventHandler<E>`] (selects [`AsyncMode`]).
/// - Structs implementing [`SyncEventHandler<E>`] (selects [`SyncMode`]).
/// - `async fn(E) -> HandlerResult` closures / function pointers (selects
///   [`AsyncFnMode`]).
/// - `fn(&E) -> HandlerResult` closures / function pointers (selects
///   [`SyncFnMode`]).
///
/// The `Mode` type parameter is inferred by the compiler from the concrete
/// handler type; callers never need to name it explicitly.
///
/// You do not need to implement this trait manually — it is a blanket
/// implementation over [`EventHandler`] and [`SyncEventHandler`].
#[allow(private_interfaces)]
pub trait IntoHandler<E: Event, Mode> {
    #[doc(hidden)]
    fn into_handler(self) -> RegisteredHandler;
}

#[allow(private_interfaces)]
impl<E, H> IntoHandler<E, AsyncMode> for H
where
    E: Event + Clone,
    H: EventHandler<E>,
{
    fn into_handler(self) -> RegisteredHandler {
        let name = self.name();
        let handler = Arc::new(self);
        let register: RegisterFn = Box::new(move |id, subscription_policy, once| {
            let typed_fn: ErasedAsyncHandlerFn = Arc::new(move |event: EventType| {
                let handler = Arc::clone(&handler);
                let event = event.downcast::<E>();
                Box::pin(async move {
                    let event = event.map_err(|_| "event type mismatch")?;
                    let event = (*event).clone();
                    handler.handle(&event).await
                })
            });
            ListenerEntry {
                id,
                kind: ListenerKind::Async(typed_fn),
                subscription_policy,
                name,
                once,
                fired: once.then(|| Arc::new(AtomicBool::new(false))),
            }
        });

        RegisteredHandler {
            register,
            name,
            is_sync: false,
        }
    }
}

#[allow(private_interfaces)]
impl<E, H> IntoHandler<E, SyncMode> for H
where
    E: Event,
    H: SyncEventHandler<E>,
{
    fn into_handler(self) -> RegisteredHandler {
        let name = self.name();
        let handler = Arc::new(self);
        let register: RegisterFn = Box::new(move |id, subscription_policy, once| {
            let typed_fn: ErasedSyncHandlerFn = Arc::new(move |event: &(dyn std::any::Any + Send + Sync)| {
                let Some(event) = event.downcast_ref::<E>() else {
                    return Err("event type mismatch".into());
                };
                handler.handle(event)
            });
            ListenerEntry {
                id,
                kind: ListenerKind::Sync(typed_fn),
                subscription_policy,
                name,
                once,
                fired: once.then(|| Arc::new(AtomicBool::new(false))),
            }
        });

        RegisteredHandler {
            register,
            name,
            is_sync: true,
        }
    }
}

#[allow(private_interfaces)]
impl<E, F> IntoHandler<E, SyncFnMode> for F
where
    E: Event,
    F: Fn(&E) -> HandlerResult + Send + Sync + 'static,
{
    fn into_handler(self) -> RegisteredHandler {
        let handler = Arc::new(self);
        let register: RegisterFn = Box::new(move |id, subscription_policy, once| {
            let typed_fn: ErasedSyncHandlerFn = Arc::new(move |event: &(dyn std::any::Any + Send + Sync)| {
                let Some(event) = event.downcast_ref::<E>() else {
                    return Err("event type mismatch".into());
                };
                handler(event)
            });
            ListenerEntry {
                id,
                kind: ListenerKind::Sync(typed_fn),
                subscription_policy,
                name: None,
                once,
                fired: once.then(|| Arc::new(AtomicBool::new(false))),
            }
        });

        RegisteredHandler {
            register,
            name: None,
            is_sync: true,
        }
    }
}

#[allow(private_interfaces)]
impl<E, F, Fut> IntoHandler<E, AsyncFnMode> for F
where
    E: Event + Clone,
    F: Fn(E) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HandlerResult> + Send + 'static,
{
    fn into_handler(self) -> RegisteredHandler {
        let handler = Arc::new(self);
        let register: RegisterFn = Box::new(move |id, subscription_policy, once| {
            let typed_fn: ErasedAsyncHandlerFn = Arc::new(move |event: EventType| {
                let handler = Arc::clone(&handler);
                let event = event.downcast::<E>();
                Box::pin(async move {
                    let event = event.map_err(|_| "event type mismatch")?;
                    let event = (*event).clone();
                    handler(event).await
                })
            });
            ListenerEntry {
                id,
                kind: ListenerKind::Async(typed_fn),
                subscription_policy,
                name: None,
                once,
                fired: once.then(|| Arc::new(AtomicBool::new(false))),
            }
        });

        RegisteredHandler {
            register,
            name: None,
            is_sync: false,
        }
    }
}

// ── HandlerDescriptor / DeadLetterDescriptor ──────────────────────────────────

/// Trait for types that can register themselves as a handler on an [`EventBus`](crate::EventBus)
/// using dependencies supplied by a [`Deps`] container.
///
/// This is the primary extension point for the builder-first registration
/// pattern. Types implementing this trait are passed to
/// [`EventBusBuilder::handler`](crate::EventBusBuilder::handler) and are
/// registered during [`build`](crate::EventBusBuilder::build).
///
/// # Manual implementation
///
/// You rarely need to implement this by hand — the `#[handler]` macro (feature
/// `macros`) generates the implementation automatically. A manual implementation
/// looks like:
///
/// ```rust,ignore
/// use std::pin::Pin;
/// use std::sync::Arc;
/// use jaeb::{Deps, EventBus, EventHandler, HandlerDescriptor, HandlerResult, Subscription, EventBusError};
///
/// #[derive(Clone)]
/// struct OrderPlaced { pub id: u64 }
///
/// struct NotifyHandler;
///
/// impl EventHandler<OrderPlaced> for NotifyHandler {
///     async fn handle(&self, event: &OrderPlaced) -> HandlerResult { Ok(()) }
/// }
///
/// impl HandlerDescriptor for NotifyHandler {
///     fn register<'a>(
///         &'a self,
///         bus: &'a EventBus,
///         _deps: &'a Deps,
///     ) -> Pin<Box<dyn std::future::Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
///         Box::pin(async move {
///             bus.subscribe::<OrderPlaced, _, _>(NotifyHandler).await
///         })
///     }
/// }
/// ```
pub trait HandlerDescriptor: Send + Sync + 'static {
    /// Register this handler on `bus`, resolving any dependencies from `deps`.
    ///
    /// Called once per descriptor during [`EventBusBuilder::build`](crate::EventBusBuilder::build).
    /// The returned [`Subscription`] is kept alive by the bus registry.
    fn register<'a>(
        &'a self,
        bus: &'a crate::bus::EventBus,
        deps: &'a Deps,
    ) -> Pin<Box<dyn Future<Output = Result<Subscription, EventBusError>> + Send + 'a>>;
}

/// Trait for types that register a **dead-letter** handler on an [`EventBus`](crate::EventBus).
///
/// This is a separate trait from [`HandlerDescriptor`] so that passing a
/// dead-letter handler to [`EventBusBuilder::handler`](crate::EventBusBuilder::handler)
/// — instead of the correct
/// [`EventBusBuilder::dead_letter`](crate::EventBusBuilder::dead_letter) — is
/// a **compile-time error**.
///
/// Dead-letter handlers must be **synchronous** (implement
/// [`SyncEventHandler<DeadLetter>`](crate::SyncEventHandler)). The `dead_letter`
/// flag on their subscription policy is forced to `false` to prevent infinite
/// recursion.
///
/// # Manual implementation
///
/// ```rust,ignore
/// use std::pin::Pin;
/// use jaeb::{DeadLetter, DeadLetterDescriptor, Deps, EventBus, EventBusError, HandlerResult, Subscription, SyncEventHandler};
///
/// struct LogDeadLetters;
///
/// impl SyncEventHandler<DeadLetter> for LogDeadLetters {
///     fn handle(&self, dl: &DeadLetter) -> HandlerResult {
///         eprintln!("dead letter: {:?}", dl);
///         Ok(())
///     }
/// }
///
/// impl DeadLetterDescriptor for LogDeadLetters {
///     fn register_dead_letter<'a>(
///         &'a self,
///         bus: &'a EventBus,
///         _deps: &'a Deps,
///     ) -> Pin<Box<dyn std::future::Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
///         Box::pin(async move {
///             bus.subscribe_dead_letters(LogDeadLetters).await
///         })
///     }
/// }
/// ```
pub trait DeadLetterDescriptor: Send + Sync + 'static {
    /// Register this dead-letter handler on `bus`, resolving any dependencies
    /// from `deps`.
    ///
    /// Called once per descriptor during [`EventBusBuilder::build`](crate::EventBusBuilder::build).
    fn register_dead_letter<'a>(
        &'a self,
        bus: &'a crate::bus::EventBus,
        deps: &'a Deps,
    ) -> Pin<Box<dyn Future<Output = Result<Subscription, EventBusError>> + Send + 'a>>;
}
