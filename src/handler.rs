use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::error::HandlerResult;
use crate::registry::{ErasedAsyncHandlerFn, ErasedSyncHandlerFn, EventType, ListenerEntry, ListenerKind};
use crate::types::Event;

pub trait EventHandler<E: Event + Clone>: Send + Sync + 'static {
    fn handle(&self, event: &E) -> impl Future<Output = HandlerResult> + Send;

    fn name(&self) -> Option<&'static str> {
        None
    }
}

pub trait SyncEventHandler<E: Event>: Send + Sync + 'static {
    fn handle(&self, event: &E) -> HandlerResult;

    fn name(&self) -> Option<&'static str> {
        None
    }
}

pub struct AsyncMode;
pub struct SyncMode;
pub struct AsyncFnMode;
pub struct SyncFnMode;

pub(crate) type RegisterFn = Box<dyn FnOnce(crate::types::SubscriptionId, crate::types::FailurePolicy, bool) -> ListenerEntry + Send>;

pub(crate) struct RegisteredHandler {
    pub register: RegisterFn,
    pub name: Option<&'static str>,
    pub is_sync: bool,
}

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
        let register: RegisterFn = Box::new(move |id, failure_policy, once| {
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
                failure_policy,
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
        let register: RegisterFn = Box::new(move |id, failure_policy, once| {
            let typed_fn: ErasedSyncHandlerFn = Arc::new(move |event: &(dyn std::any::Any + Send + Sync)| {
                let Some(event) = event.downcast_ref::<E>() else {
                    return Err("event type mismatch".into());
                };
                handler.handle(event)
            });
            ListenerEntry {
                id,
                kind: ListenerKind::Sync(typed_fn),
                failure_policy,
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
        let register: RegisterFn = Box::new(move |id, failure_policy, once| {
            let typed_fn: ErasedSyncHandlerFn = Arc::new(move |event: &(dyn std::any::Any + Send + Sync)| {
                let Some(event) = event.downcast_ref::<E>() else {
                    return Err("event type mismatch".into());
                };
                (handler)(event)
            });
            ListenerEntry {
                id,
                kind: ListenerKind::Sync(typed_fn),
                failure_policy,
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
        let register: RegisterFn = Box::new(move |id, failure_policy, once| {
            let typed_fn: ErasedAsyncHandlerFn = Arc::new(move |event: EventType| {
                let handler = Arc::clone(&handler);
                let event = event.downcast::<E>();
                Box::pin(async move {
                    let event = event.map_err(|_| "event type mismatch")?;
                    let event = (*event).clone();
                    (handler)(event).await
                })
            });
            ListenerEntry {
                id,
                kind: ListenerKind::Async(typed_fn),
                failure_policy,
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
