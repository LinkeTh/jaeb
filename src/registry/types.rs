use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::HandlerResult;
use crate::middleware::MiddlewareDecision;
use crate::types::{AsyncSubscriptionPolicy, SubscriptionId, SyncSubscriptionPolicy};

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

#[derive(Clone, Copy)]
pub(crate) enum ListenerPolicy {
    Async(AsyncSubscriptionPolicy),
    Sync(SyncSubscriptionPolicy),
}

impl From<AsyncSubscriptionPolicy> for ListenerPolicy {
    fn from(policy: AsyncSubscriptionPolicy) -> Self {
        Self::Async(policy)
    }
}

impl From<SyncSubscriptionPolicy> for ListenerPolicy {
    fn from(policy: SyncSubscriptionPolicy) -> Self {
        Self::Sync(policy)
    }
}

#[derive(Clone)]
pub(crate) struct ListenerEntry {
    pub id: SubscriptionId,
    pub registration_order: u64,
    pub kind: ListenerKind,
    pub subscription_policy: ListenerPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone)]
pub(crate) struct AsyncListenerEntry {
    pub id: SubscriptionId,
    pub registration_order: u64,
    pub handler: ErasedAsyncHandlerFn,
    pub subscription_policy: AsyncSubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl From<&ListenerEntry> for AsyncListenerEntry {
    fn from(entry: &ListenerEntry) -> Self {
        let ListenerKind::Async(handler) = &entry.kind else {
            panic!("Cannot convert Sync listener to AsyncListenerEntry");
        };
        let ListenerPolicy::Async(subscription_policy) = entry.subscription_policy else {
            panic!("Cannot convert Sync listener policy to AsyncListenerEntry");
        };

        Self {
            id: entry.id,
            registration_order: entry.registration_order,
            handler: Arc::clone(handler),
            subscription_policy,
            name: entry.name,
            once: entry.once,
            fired: entry.fired.as_ref().map(Arc::clone),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SyncListenerEntry {
    pub id: SubscriptionId,
    pub registration_order: u64,
    pub handler: ErasedSyncHandlerFn,
    pub subscription_policy: SyncSubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl From<&ListenerEntry> for SyncListenerEntry {
    fn from(entry: &ListenerEntry) -> Self {
        let ListenerKind::Sync(handler) = &entry.kind else {
            panic!("Cannot convert Async listener to SyncListenerEntry");
        };
        let ListenerPolicy::Sync(subscription_policy) = entry.subscription_policy else {
            panic!("Cannot convert Async listener policy to SyncListenerEntry");
        };

        Self {
            id: entry.id,
            registration_order: entry.registration_order,
            handler: Arc::clone(handler),
            subscription_policy,
            name: entry.name,
            once: entry.once,
            fired: entry.fired.as_ref().map(Arc::clone),
        }
    }
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
    pub sync_listeners: Box<[SyncListenerEntry]>,
    pub async_listeners: Box<[AsyncListenerEntry]>,
    pub middlewares: Box<[TypedMiddlewareSlot]>,
    pub has_async_middleware: bool,
    pub sync_gate: Arc<Mutex<()>>,
}

#[derive(Clone)]
pub(crate) struct DeadLetterSlot {
    pub listeners: Box<[SyncListenerEntry]>,
    pub sync_gate: Arc<Mutex<()>>,
}

#[derive(Default)]
pub(crate) struct RegistrySnapshot {
    pub by_type: HashMap<TypeId, Arc<TypeSlot>>,
    pub dead_letter: Option<Arc<DeadLetterSlot>>,
    pub global_middlewares: Box<[(SubscriptionId, ErasedMiddleware)]>,
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

pub(crate) struct DispatchContext<'a> {
    pub bus: &'a crate::bus::EventBus,
    pub spawn_async_handlers: bool,
    /// When the `trace` feature is enabled, this captures the caller's tracing
    /// span at publish time so enqueued async handler work inherits the correct
    /// parent context automatically.
    #[cfg(feature = "trace")]
    pub parent_span: tracing::Span,
}
