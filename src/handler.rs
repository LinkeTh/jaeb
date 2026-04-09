// SPDX-License-Identifier: MIT
use std::future::Future;
use std::sync::Arc;

use crate::actor::{ListenerMeta, ListenerRegistry, RegisterFn, TypedAsyncHandlerFn, TypedHandler, TypedSyncHandlerFn};
use crate::error::HandlerResult;
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
        let register: RegisterFn = Box::new(move |registry: &mut ListenerRegistry, meta: ListenerMeta| {
            let typed_fn: TypedAsyncHandlerFn<E> = Arc::new(move |event: Arc<E>| {
                let handler = Arc::clone(&handler);
                let event = (*event).clone();
                Box::pin(async move { handler.handle(&event).await })
            });
            registry.add_typed_listener::<E>(meta, TypedHandler::Async(typed_fn));
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
        let register: RegisterFn = Box::new(move |registry: &mut ListenerRegistry, meta: ListenerMeta| {
            let typed_fn: TypedSyncHandlerFn<E> = Arc::new(move |event: &E| handler.handle(event));
            registry.add_typed_listener::<E>(meta, TypedHandler::Sync(typed_fn));
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
        let register: RegisterFn = Box::new(move |registry: &mut ListenerRegistry, meta: ListenerMeta| {
            let typed_fn: TypedSyncHandlerFn<E> = Arc::new(move |event: &E| (handler)(event));
            registry.add_typed_listener::<E>(meta, TypedHandler::Sync(typed_fn));
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
        let register: RegisterFn = Box::new(move |registry: &mut ListenerRegistry, meta: ListenerMeta| {
            let typed_fn: TypedAsyncHandlerFn<E> = Arc::new(move |event: Arc<E>| {
                let handler = Arc::clone(&handler);
                let event = (*event).clone();
                Box::pin(async move { (handler)(event).await })
            });
            registry.add_typed_listener::<E>(meta, TypedHandler::Async(typed_fn));
        });

        RegisteredHandler {
            register,
            name: None,
            is_sync: false,
        }
    }
}
