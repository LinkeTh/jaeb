use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};

use crate::error::HandlerResult;
use crate::middleware::MiddlewareDecision;
use crate::types::SubscriptionId;

pub(crate) type EventType = Arc<dyn Any + Send + Sync>;
pub(crate) type HandlerFuture = Pin<Box<dyn Future<Output = HandlerResult> + Send>>;
pub(crate) type MiddlewareFuture = Pin<Box<dyn Future<Output = MiddlewareDecision> + Send>>;

pub(crate) type ErasedAsyncMiddleware = Arc<dyn Fn(&'static str, EventType) -> MiddlewareFuture + Send + Sync>;
pub(crate) type ErasedSyncMiddleware = Arc<dyn Fn(&'static str, &(dyn Any + Send + Sync)) -> MiddlewareDecision + Send + Sync>;

#[derive(Clone)]
pub(crate) enum ErasedMiddleware {
    Async(ErasedAsyncMiddleware),
    Sync(ErasedSyncMiddleware),
}

pub(crate) type ErasedAsyncHandlerFn = Arc<dyn Fn(EventType) -> HandlerFuture + Send + Sync + 'static>;
pub(crate) type ErasedSyncHandlerFn = Arc<dyn Fn(&(dyn Any + Send + Sync)) -> HandlerResult + Send + Sync + 'static>;

pub(crate) type ErasedTypedAsyncMiddlewareFn = Arc<dyn Fn(&'static str, EventType) -> MiddlewareFuture + Send + Sync + 'static>;
pub(crate) type ErasedTypedSyncMiddlewareFn = Arc<dyn Fn(&'static str, &(dyn Any + Send + Sync)) -> MiddlewareDecision + Send + Sync + 'static>;

#[derive(Clone)]
pub(crate) enum ListenerKind {
    Async(ErasedAsyncHandlerFn),
    Sync(ErasedSyncHandlerFn),
}

#[derive(Clone)]
pub(crate) struct ListenerEntry {
    pub id: SubscriptionId,
    pub kind: ListenerKind,
    pub subscription_policy: crate::types::SubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone)]
pub(crate) struct AsyncListenerEntry {
    pub id: SubscriptionId,
    pub handler: ErasedAsyncHandlerFn,
    pub subscription_policy: crate::types::SubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone)]
pub(crate) struct SyncListenerEntry {
    pub id: SubscriptionId,
    pub handler: ErasedSyncHandlerFn,
    pub subscription_policy: crate::types::SubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone)]
pub(crate) enum TypedMiddlewareEntry {
    Async(ErasedTypedAsyncMiddlewareFn),
    Sync(ErasedTypedSyncMiddlewareFn),
}

#[derive(Clone)]
pub(crate) struct TypedMiddlewareSlot {
    pub id: SubscriptionId,
    pub middleware: TypedMiddlewareEntry,
}

#[derive(Clone)]
pub(crate) struct TypeSlot {
    pub sync_listeners: Arc<[SyncListenerEntry]>,
    pub async_listeners: Arc<[AsyncListenerEntry]>,
    pub middlewares: Arc<[TypedMiddlewareSlot]>,
    pub has_async_middleware: bool,
    pub sync_gate: Arc<Mutex<()>>,
    pub async_semaphore: Option<Arc<Semaphore>>,
}

#[derive(Default)]
pub(crate) struct RegistrySnapshot {
    pub by_type: HashMap<TypeId, Arc<TypeSlot>>,
    pub global_middlewares: Arc<[(SubscriptionId, ErasedMiddleware)]>,
    pub global_has_async_middleware: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ListenerFailure {
    pub event_name: &'static str,
    pub subscription_id: SubscriptionId,
    pub attempts: usize,
    pub error: String,
    pub dead_letter: bool,
    pub event: EventType,
    pub handler_name: Option<&'static str>,
}

pub(crate) enum ControlNotification {
    Failure(ListenerFailure),
    Flush(oneshot::Sender<()>),
}

pub(crate) struct DispatchContext<'a> {
    pub tracker: &'a Arc<super::tracker::AsyncTaskTracker>,
    pub notify_tx: &'a mpsc::UnboundedSender<ControlNotification>,
    pub handler_timeout: Option<Duration>,
    pub spawn_async_handlers: bool,
}
