use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use tokio::sync::{Mutex, Notify, Semaphore, mpsc, oneshot};
use tokio::task::AbortHandle;
#[cfg(feature = "trace")]
use tracing::{error, warn};

#[cfg(feature = "metrics")]
use metrics::counter;

use crate::error::{EventBusError, HandlerResult};
use crate::middleware::MiddlewareDecision;
use crate::types::{DeadLetter, ListenerInfo, SubscriptionId, SubscriptionPolicy};

#[cfg(feature = "metrics")]
use crate::metrics::TimerGuard;

pub(crate) type EventType = Arc<dyn Any + Send + Sync>;
pub(crate) type HandlerFuture = Pin<Box<dyn Future<Output = HandlerResult> + Send>>;
pub(crate) type MiddlewareFuture = Pin<Box<dyn Future<Output = MiddlewareDecision> + Send>>;

fn extract_panic_message(panic_payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = panic_payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "handler panicked".to_string()
    }
}

struct CatchUnwindFuture {
    inner: HandlerFuture,
}

impl CatchUnwindFuture {
    fn new(inner: HandlerFuture) -> Self {
        Self { inner }
    }
}

impl Future for CatchUnwindFuture {
    type Output = Result<HandlerResult, String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        match catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(cx))) {
            Ok(Poll::Ready(result)) => Poll::Ready(Ok(result)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(panic_payload) => Poll::Ready(Err(extract_panic_message(panic_payload))),
        }
    }
}

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
    pub subscription_policy: SubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<AtomicBool>>,
}

#[derive(Clone)]
pub(crate) struct AsyncListenerEntry {
    pub id: SubscriptionId,
    pub handler: ErasedAsyncHandlerFn,
    pub subscription_policy: SubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<AtomicBool>>,
}

#[derive(Clone)]
pub(crate) struct SyncListenerEntry {
    pub id: SubscriptionId,
    pub handler: ErasedSyncHandlerFn,
    pub subscription_policy: SubscriptionPolicy,
    pub name: Option<&'static str>,
    pub once: bool,
    pub fired: Option<Arc<AtomicBool>>,
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
    pub sync_gate: Arc<Mutex<()>>,
    pub async_semaphore: Option<Arc<Semaphore>>,
}

#[derive(Default)]
pub(crate) struct RegistrySnapshot {
    pub by_type: HashMap<TypeId, Arc<TypeSlot>>,
    pub global_middlewares: Arc<[(SubscriptionId, ErasedMiddleware)]>,
}

struct MutableTypeSlot {
    event_name: &'static str,
    listeners: Vec<ListenerEntry>,
    middlewares: Vec<TypedMiddlewareSlot>,
    sync_gate: Arc<Mutex<()>>,
    async_semaphore: Option<Arc<Semaphore>>,
}

impl MutableTypeSlot {
    fn to_snapshot_slot(&self) -> Arc<TypeSlot> {
        let mut sync_listeners: Vec<SyncListenerEntry> = Vec::new();
        let mut async_listeners: Vec<AsyncListenerEntry> = Vec::new();
        for listener in &self.listeners {
            match &listener.kind {
                ListenerKind::Sync(handler) => sync_listeners.push(SyncListenerEntry {
                    id: listener.id,
                    handler: Arc::clone(handler),
                    subscription_policy: listener.subscription_policy,
                    name: listener.name,
                    once: listener.once,
                    fired: listener.fired.as_ref().map(Arc::clone),
                }),
                ListenerKind::Async(handler) => async_listeners.push(AsyncListenerEntry {
                    id: listener.id,
                    handler: Arc::clone(handler),
                    subscription_policy: listener.subscription_policy,
                    name: listener.name,
                    once: listener.once,
                    fired: listener.fired.as_ref().map(Arc::clone),
                }),
            }
        }
        sync_listeners.sort_by(|a, b| b.subscription_policy.priority.cmp(&a.subscription_policy.priority));
        async_listeners.sort_by(|a, b| b.subscription_policy.priority.cmp(&a.subscription_policy.priority));

        Arc::new(TypeSlot {
            sync_listeners: sync_listeners.into(),
            async_listeners: async_listeners.into(),
            middlewares: self.middlewares.clone().into(),
            sync_gate: Arc::clone(&self.sync_gate),
            async_semaphore: self.async_semaphore.as_ref().map(Arc::clone),
        })
    }
}

enum IndexEntry {
    Listener(TypeId),
    TypedMiddleware(TypeId),
    GlobalMiddleware,
}

pub(crate) struct MutableRegistry {
    slots: HashMap<TypeId, MutableTypeSlot>,
    global_middlewares: Vec<(SubscriptionId, ErasedMiddleware)>,
    index: HashMap<SubscriptionId, IndexEntry>,
    type_names: HashMap<TypeId, &'static str>,
    max_concurrent_async: Option<usize>,
}

impl MutableRegistry {
    pub(crate) fn new(max_concurrent_async: Option<usize>) -> Self {
        Self {
            slots: HashMap::new(),
            global_middlewares: Vec::new(),
            index: HashMap::new(),
            type_names: HashMap::new(),
            max_concurrent_async,
        }
    }

    fn ensure_slot(&mut self, event_type: TypeId, event_name: &'static str) -> &mut MutableTypeSlot {
        self.type_names.entry(event_type).or_insert(event_name);
        self.slots.entry(event_type).or_insert_with(|| MutableTypeSlot {
            event_name,
            listeners: Vec::new(),
            middlewares: Vec::new(),
            sync_gate: Arc::new(Mutex::new(())),
            async_semaphore: self.max_concurrent_async.map(|n| Arc::new(Semaphore::new(n))),
        })
    }

    pub(crate) fn add_listener(&mut self, event_type: TypeId, event_name: &'static str, listener: ListenerEntry) {
        self.ensure_slot(event_type, event_name).listeners.push(listener.clone());
        self.index.insert(listener.id, IndexEntry::Listener(event_type));
    }

    pub(crate) fn add_typed_middleware(&mut self, event_type: TypeId, event_name: &'static str, middleware: TypedMiddlewareSlot) {
        self.ensure_slot(event_type, event_name).middlewares.push(middleware.clone());
        self.index.insert(middleware.id, IndexEntry::TypedMiddleware(event_type));
    }

    pub(crate) fn add_global_middleware(&mut self, id: SubscriptionId, middleware: ErasedMiddleware) {
        self.global_middlewares.push((id, middleware));
        self.index.insert(id, IndexEntry::GlobalMiddleware);
    }

    pub(crate) fn remove_once(&mut self, subscription_id: SubscriptionId) {
        let Some(IndexEntry::Listener(event_type)) = self.index.get(&subscription_id) else {
            return;
        };
        if let Some(slot) = self.slots.get_mut(event_type) {
            slot.listeners.retain(|l| l.id != subscription_id);
            if slot.listeners.is_empty() && slot.middlewares.is_empty() {
                self.slots.remove(event_type);
                self.type_names.remove(event_type);
            }
        }
        self.index.remove(&subscription_id);
    }

    pub(crate) fn remove_subscription(&mut self, subscription_id: SubscriptionId) -> bool {
        match self.index.remove(&subscription_id) {
            Some(IndexEntry::GlobalMiddleware) => {
                let before = self.global_middlewares.len();
                self.global_middlewares.retain(|(id, _)| *id != subscription_id);
                before != self.global_middlewares.len()
            }
            Some(IndexEntry::Listener(event_type)) => {
                if let Some(slot) = self.slots.get_mut(&event_type) {
                    let before = slot.listeners.len();
                    slot.listeners.retain(|l| l.id != subscription_id);
                    let removed = before != slot.listeners.len();
                    if slot.listeners.is_empty() && slot.middlewares.is_empty() {
                        self.slots.remove(&event_type);
                        self.type_names.remove(&event_type);
                    }
                    removed
                } else {
                    false
                }
            }
            Some(IndexEntry::TypedMiddleware(event_type)) => {
                if let Some(slot) = self.slots.get_mut(&event_type) {
                    let before = slot.middlewares.len();
                    slot.middlewares.retain(|m| m.id != subscription_id);
                    let removed = before != slot.middlewares.len();
                    if slot.listeners.is_empty() && slot.middlewares.is_empty() {
                        self.slots.remove(&event_type);
                        self.type_names.remove(&event_type);
                    }
                    removed
                } else {
                    false
                }
            }
            None => false,
        }
    }

    pub(crate) fn snapshot(&self) -> RegistrySnapshot {
        let mut by_type = HashMap::with_capacity(self.slots.len());
        for (type_id, slot) in &self.slots {
            by_type.insert(*type_id, slot.to_snapshot_slot());
        }
        RegistrySnapshot {
            by_type,
            global_middlewares: self.global_middlewares.clone().into(),
        }
    }

    pub(crate) fn stats(&self, in_flight_async: usize, queue_capacity: usize, shutdown_called: bool) -> crate::types::BusStats {
        let mut subscriptions_by_event: HashMap<&'static str, Vec<ListenerInfo>> = HashMap::new();
        let mut total_subscriptions = 0usize;
        let mut registered_event_types = Vec::new();

        for (type_id, slot) in &self.slots {
            if slot.listeners.is_empty() {
                continue;
            }
            let event_name = self.type_names.get(type_id).copied().unwrap_or(slot.event_name);
            registered_event_types.push(event_name);
            let infos: Vec<ListenerInfo> = slot
                .listeners
                .iter()
                .map(|l| ListenerInfo {
                    subscription_id: l.id,
                    name: l.name,
                })
                .collect();
            total_subscriptions += infos.len();
            subscriptions_by_event.insert(event_name, infos);
        }

        registered_event_types.sort_unstable();

        crate::types::BusStats {
            total_subscriptions,
            subscriptions_by_event,
            registered_event_types,
            queue_capacity,
            in_flight_async,
            shutdown_called,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ListenerFailure {
    pub event_name: &'static str,
    pub subscription_id: SubscriptionId,
    pub attempts: usize,
    pub error: String,
    pub dead_letter: bool,
    pub event: EventType,
    pub listener_name: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct AsyncListenerMeta {
    subscription_id: SubscriptionId,
    subscription_policy: SubscriptionPolicy,
    listener_name: Option<&'static str>,
}

pub(crate) enum ControlNotification {
    Failure(ListenerFailure),
    Flush(oneshot::Sender<()>),
}

pub(crate) struct DispatchContext<'a> {
    pub tracker: &'a Arc<AsyncTaskTracker>,
    pub notify_tx: &'a mpsc::UnboundedSender<ControlNotification>,
    pub handler_timeout: Option<Duration>,
    pub spawn_async_handlers: bool,
}

pub(crate) struct AsyncTaskTracker {
    next_id: AtomicU64,
    in_flight: AtomicUsize,
    tasks: Option<StdMutex<HashMap<u64, AbortHandle>>>,
    notify: Notify,
}

impl Default for AsyncTaskTracker {
    fn default() -> Self {
        Self::new(true)
    }
}

impl AsyncTaskTracker {
    pub(crate) fn new(track_abort_handles: bool) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            in_flight: AtomicUsize::new(0),
            tasks: track_abort_handles.then(|| StdMutex::new(HashMap::new())),
            notify: Notify::new(),
        }
    }

    pub(crate) fn spawn_tracked<F>(self: &Arc<Self>, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        let tracker = Arc::clone(self);

        if let Some(tasks) = &self.tasks {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let mut guard = tasks.lock().expect("tracker task lock poisoned");

            // Keep the map lock across spawn so the abort handle is inserted
            // before the task can complete and attempt removal.
            let handle = tokio::spawn(async move {
                fut.await;
                tracker.finish_task(Some(id));
            });

            guard.insert(id, handle.abort_handle());
        } else {
            tokio::spawn(async move {
                fut.await;
                tracker.finish_task(None);
            });
        }
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    pub(crate) async fn shutdown(&self, timeout: Option<Duration>) -> bool {
        if self.in_flight() == 0 {
            return false;
        }

        if let Some(timeout) = timeout {
            let wait = async {
                loop {
                    let notified = self.notify.notified();
                    if self.in_flight() == 0 {
                        return;
                    }
                    notified.await;
                }
            };
            if tokio::time::timeout(timeout, wait).await.is_err() {
                debug_assert!(
                    self.tasks.is_some(),
                    "shutdown timeout requires tracked abort handles to cancel in-flight tasks"
                );
                let handles: Vec<AbortHandle> = self
                    .tasks
                    .as_ref()
                    .map(|tasks| {
                        let mut guard = tasks.lock().expect("tracker task lock poisoned");
                        guard.drain().map(|(_, h)| h).collect()
                    })
                    .unwrap_or_default();
                for handle in &handles {
                    handle.abort();
                }
                return true;
            }
            false
        } else {
            loop {
                let notified = self.notify.notified();
                if self.in_flight() == 0 {
                    break;
                }
                notified.await;
            }
            false
        }
    }

    fn finish_task(&self, id: Option<u64>) {
        let prev = self.in_flight.fetch_sub(1, Ordering::AcqRel);
        if let Some(id) = id {
            self.remove_abort_handle(id);
        }
        if prev == 1 {
            self.notify.notify_waiters();
        }
    }

    fn remove_abort_handle(&self, id: u64) {
        if let Some(tasks) = &self.tasks {
            tasks.lock().expect("tracker task lock poisoned").remove(&id);
        }
    }
}

fn sync_listener_failed(listener: &SyncListenerEntry, event_name: &'static str, event: &EventType, err: String) -> ListenerFailure {
    ListenerFailure {
        event_name,
        subscription_id: listener.id,
        attempts: 1,
        error: err,
        dead_letter: listener.subscription_policy.dead_letter,
        event: Arc::clone(event),
        listener_name: listener.name,
    }
}

async fn execute_async_listener(
    handler: ErasedAsyncHandlerFn,
    event: EventType,
    event_name: &'static str,
    listener: AsyncListenerMeta,
    handler_timeout: Option<Duration>,
) -> Option<ListenerFailure> {
    let mut retries_left = listener.subscription_policy.max_retries;
    let mut attempts = 0usize;
    loop {
        attempts += 1;

        #[cfg(feature = "metrics")]
        let _timer = TimerGuard::start("eventbus.handler.duration", event_name, listener.listener_name);

        let handler_future = CatchUnwindFuture::new(handler(Arc::clone(&event)));

        let result = match handler_timeout {
            Some(timeout) => match tokio::time::timeout(timeout, handler_future).await {
                Ok(Ok(inner)) => inner,
                Ok(Err(panic_msg)) => Err(format!("handler panicked: {panic_msg}").into()),
                Err(_) => Err(format!("handler timed out after {timeout:?}").into()),
            },
            None => match handler_future.await {
                Ok(inner) => inner,
                Err(panic_msg) => Err(format!("handler panicked: {panic_msg}").into()),
            },
        };

        match result {
            Ok(()) => return None,
            Err(err) => {
                let error_message = err.to_string();
                if retries_left == 0 {
                    return Some(ListenerFailure {
                        event_name,
                        subscription_id: listener.subscription_id,
                        attempts,
                        error: error_message,
                        dead_letter: listener.subscription_policy.dead_letter,
                        event: Arc::clone(&event),
                        listener_name: listener.listener_name,
                    });
                }

                retries_left -= 1;
                #[cfg(feature = "trace")]
                warn!(
                    event = event_name,
                    listener_id = listener.subscription_id.as_u64(),
                    listener_name = listener.listener_name,
                    attempts,
                    retries_left,
                    error = %error_message,
                    "handler.retry"
                );

                if let Some(strategy) = listener.subscription_policy.retry_strategy {
                    tokio::time::sleep(strategy.delay_for_attempt(attempts - 1)).await;
                }
            }
        }
    }
}

fn should_fire_once(once: bool, fired: Option<&Arc<AtomicBool>>) -> bool {
    if !once {
        return true;
    }
    if let Some(flag) = fired {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
    } else {
        true
    }
}

async fn dispatch_slot(slot: &TypeSlot, event: &EventType, event_name: &'static str, dispatch_ctx: &DispatchContext<'_>) -> Vec<SubscriptionId> {
    let mut once_removed = Vec::new();

    // Async lane: schedule async handlers without waiting for sync gate.
    if dispatch_ctx.spawn_async_handlers {
        let tracker = Arc::clone(dispatch_ctx.tracker);
        let handler_timeout = dispatch_ctx.handler_timeout;
        let semaphore = slot.async_semaphore.as_ref().map(Arc::clone);
        for listener in slot.async_listeners.iter() {
            if !should_fire_once(listener.once, listener.fired.as_ref()) {
                continue;
            }
            if listener.once {
                once_removed.push(listener.id);
            }

            let listener_meta = AsyncListenerMeta {
                subscription_id: listener.id,
                subscription_policy: listener.subscription_policy,
                listener_name: listener.name,
            };
            let handler = Arc::clone(&listener.handler);
            let event = Arc::clone(event);
            let notify = dispatch_ctx.notify_tx.clone();
            let semaphore = semaphore.as_ref().map(Arc::clone);
            tracker.spawn_tracked(async move {
                let task = async {
                    if let Some(failure) = execute_async_listener(handler, event, event_name, listener_meta, handler_timeout).await {
                        let _ = notify.send(ControlNotification::Failure(failure));
                    }
                };

                if let Some(semaphore) = semaphore {
                    if let Ok(_permit) = semaphore.acquire().await {
                        task.await;
                    }
                } else {
                    task.await;
                }
            });
        }
    }

    if slot.sync_listeners.is_empty() {
        return once_removed;
    }

    // Sync lane: serialized FIFO for sync handlers only.
    let _guard = slot.sync_gate.lock().await;
    for listener in slot.sync_listeners.iter() {
        if !should_fire_once(listener.once, listener.fired.as_ref()) {
            continue;
        }
        if listener.once {
            once_removed.push(listener.id);
        }

        #[cfg(feature = "metrics")]
        let _timer = TimerGuard::start("eventbus.handler.duration", event_name, listener.name);

        let result = catch_unwind(AssertUnwindSafe(|| {
            // Safe because we only pass shared references to listener code.
            (listener.handler)(event.as_ref())
        }))
        .unwrap_or_else(|panic_payload| Err(extract_panic_message(panic_payload).into()));

        if let Err(err) = result {
            let _ = dispatch_ctx.notify_tx.send(ControlNotification::Failure(sync_listener_failed(
                listener,
                event_name,
                event,
                err.to_string(),
            )));
        }
    }

    once_removed
}

pub(crate) async fn dispatch_with_snapshot(
    snapshot: &RegistrySnapshot,
    event_type: TypeId,
    event: EventType,
    event_name: &'static str,
    dispatch_ctx: &DispatchContext<'_>,
) -> Result<Vec<SubscriptionId>, EventBusError> {
    #[cfg(feature = "metrics")]
    counter!("eventbus.publish", "event" => event_name).increment(1);

    let slot = snapshot.by_type.get(&event_type);
    if snapshot.global_middlewares.is_empty() {
        let Some(slot) = slot else {
            return Ok(Vec::new());
        };
        if slot.middlewares.is_empty() {
            return Ok(dispatch_slot(slot.as_ref(), &event, event_name, dispatch_ctx).await);
        }
    } else {
        for (_id, mw) in snapshot.global_middlewares.iter() {
            let decision = match mw {
                ErasedMiddleware::Async(f) => f(event_name, Arc::clone(&event)).await,
                ErasedMiddleware::Sync(f) => f(event_name, event.as_ref()),
            };
            if let MiddlewareDecision::Reject(reason) = decision {
                return Err(EventBusError::MiddlewareRejected(reason));
            }
        }
    }

    let Some(slot) = slot else {
        return Ok(Vec::new());
    };

    if !slot.middlewares.is_empty() {
        for slot_mw in slot.middlewares.iter() {
            let decision = match &slot_mw.middleware {
                TypedMiddlewareEntry::Async(mw) => mw(event_name, Arc::clone(&event)).await,
                TypedMiddlewareEntry::Sync(mw) => mw(event_name, event.as_ref()),
            };
            if let MiddlewareDecision::Reject(reason) = decision {
                return Err(EventBusError::MiddlewareRejected(reason));
            }
        }
    }

    Ok(dispatch_slot(slot.as_ref(), &event, event_name, dispatch_ctx).await)
}

pub(crate) fn dead_letter_from_failure(failure: &ListenerFailure) -> Option<DeadLetter> {
    #[cfg(feature = "trace")]
    error!(
        event = failure.event_name,
        listener_id = failure.subscription_id.as_u64(),
        listener_name = failure.listener_name,
        attempts = failure.attempts,
        error = %failure.error,
        "handler.failed"
    );

    #[cfg(feature = "metrics")]
    if let Some(name) = failure.listener_name {
        counter!("eventbus.handler.error", "event" => failure.event_name, "listener" => name).increment(1);
    } else {
        counter!("eventbus.handler.error", "event" => failure.event_name).increment(1);
    }

    let dead_letter_type = std::any::type_name::<DeadLetter>();
    if failure.dead_letter && failure.event_name != dead_letter_type {
        Some(DeadLetter {
            event_name: failure.event_name,
            subscription_id: failure.subscription_id,
            attempts: failure.attempts,
            error: failure.error.clone(),
            event: failure.event.clone(),
            failed_at: std::time::SystemTime::now(),
            listener_name: failure.listener_name,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::AsyncTaskTracker;
    use std::sync::Arc;
    use tokio::sync::Barrier;

    #[tokio::test]
    async fn tracker_does_not_leak_handles_for_fast_tasks() {
        let tracker = Arc::new(AsyncTaskTracker::new(true));
        let barrier = Arc::new(Barrier::new(2));

        tracker.spawn_tracked({
            let barrier = Arc::clone(&barrier);
            async move {
                barrier.wait().await;
            }
        });

        barrier.wait().await;
        let timed_out = tracker.shutdown(Some(std::time::Duration::from_secs(1))).await;
        assert!(!timed_out, "tracker shutdown should complete without timeout");

        let guard = tracker
            .tasks
            .as_ref()
            .expect("tracker should track abort handles in this test")
            .lock()
            .expect("tracker task lock poisoned");
        assert!(guard.is_empty(), "tracker task map should be empty after completion");
    }
}
