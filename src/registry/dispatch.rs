use std::any::{Any, TypeId};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use futures_util::FutureExt;
#[cfg(feature = "trace")]
use tracing::{Instrument, error, warn};

use super::types::{
    AsyncListenerEntry, DispatchContext, ErasedAsyncHandlerFn, ErasedMiddleware, EventType, HandlerFuture, ListenerFailure, RegistrySnapshot,
    SyncListenerEntry, TypeSlot, TypedMiddlewareEntry,
};
use crate::AsyncSubscriptionPolicy;
use crate::bus::EventBus;
use crate::error::{EventBusError, HandlerResult};
use crate::middleware::MiddlewareDecision;
use crate::types::{DeadLetter, SubscriptionId};
#[cfg(feature = "metrics")]
use metrics::counter;

#[cfg(feature = "metrics")]
use crate::metrics::TimerGuard;

fn extract_panic_message(panic_payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = panic_payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "handler panicked".to_string()
    }
}

fn middleware_panic_error(panic_payload: Box<dyn Any + Send>) -> EventBusError {
    EventBusError::MiddlewareRejected(format!("middleware panicked: {}", extract_panic_message(panic_payload)))
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

#[derive(Clone, Copy)]
pub(crate) struct AsyncListenerMeta {
    pub subscription_id: SubscriptionId,
    pub subscription_policy: AsyncSubscriptionPolicy,
    pub handler_name: Option<&'static str>,
}

fn sync_listener_failed(listener: &SyncListenerEntry, event_name: &'static str, event: &EventType, err: String) -> ListenerFailure {
    ListenerFailure {
        event_name,
        subscription_id: listener.id,
        attempts: 1,
        error: err,
        dead_letter: listener.subscription_policy.dead_letter,
        event: Arc::clone(event),
        handler_name: listener.name,
    }
}

pub(crate) async fn finalize_listener_failure(bus: &EventBus, failure: ListenerFailure) {
    if let Some(dead_letter) = dead_letter_from_failure(&failure) {
        let snapshot = bus.inner.snapshot.load_full();
        deliver_dead_letter(bus, snapshot.as_ref(), dead_letter).await;
    }
}

async fn deliver_dead_letter(bus: &EventBus, snapshot: &RegistrySnapshot, dead_letter: DeadLetter) {
    let event_name = std::any::type_name::<DeadLetter>();

    #[cfg(feature = "metrics")]
    counter!("eventbus.publish", "event" => event_name).increment(1);

    let event = Arc::new(dead_letter) as EventType;

    if !snapshot.global_middlewares.is_empty() {
        for (_id, mw) in snapshot.global_middlewares.iter() {
            let decision = match mw {
                ErasedMiddleware::Async(f) => match AssertUnwindSafe(f(event_name, Arc::clone(&event))).catch_unwind().await {
                    Ok(decision) => decision,
                    Err(_) => return,
                },
                ErasedMiddleware::Sync(f) => match catch_unwind(AssertUnwindSafe(|| f(event_name, event.as_ref()))) {
                    Ok(decision) => decision,
                    Err(_) => return,
                },
            };
            if let MiddlewareDecision::Reject(_) = decision {
                return;
            }
        }
    }

    if let Some(slot) = snapshot.by_type.get(&TypeId::of::<DeadLetter>())
        && !slot.middlewares.is_empty()
    {
        for slot_mw in slot.middlewares.iter() {
            let decision = match &slot_mw.middleware {
                TypedMiddlewareEntry::Async(mw) => match AssertUnwindSafe(mw(event_name, Arc::clone(&event))).catch_unwind().await {
                    Ok(decision) => decision,
                    Err(_) => return,
                },
                TypedMiddlewareEntry::Sync(mw) => match catch_unwind(AssertUnwindSafe(|| mw(event_name, event.as_ref()))) {
                    Ok(decision) => decision,
                    Err(_) => return,
                },
            };
            if let MiddlewareDecision::Reject(_) = decision {
                return;
            }
        }
    }

    let Some(slot) = snapshot.dead_letter.as_deref() else {
        return;
    };

    let mut once_removed = Vec::new();
    let _guard = slot.sync_gate.lock().await;
    for listener in slot.listeners.iter() {
        if !should_fire_once(listener.once, listener.fired.as_ref()) {
            continue;
        }
        if listener.once {
            once_removed.push(listener.id);
        }

        #[cfg(feature = "metrics")]
        let _timer = TimerGuard::start("eventbus.handler.duration", event_name, listener.name);

        let result = catch_unwind(AssertUnwindSafe(|| (listener.handler)(event.as_ref())))
            .unwrap_or_else(|panic_payload| Err(extract_panic_message(panic_payload).into()));

        if let Err(_err) = result {
            #[cfg(feature = "trace")]
            error!(
                event = event_name,
                listener_id = listener.id.as_u64(),
                handler_name = listener.name,
                error = %_err,
                "dead_letter_handler.failed"
            );

            #[cfg(feature = "metrics")]
            if let Some(name) = listener.name {
                counter!("eventbus.handler.error", "event" => event_name, "listener" => name).increment(1);
            } else {
                counter!("eventbus.handler.error", "event" => event_name).increment(1);
            }
        }
    }

    if !once_removed.is_empty() {
        let mut registry = bus.inner.registry.lock().await;
        for subscription_id in once_removed {
            registry.remove_once(subscription_id);
        }
        bus.refresh_snapshot_locked(&registry).await;
    }
}

pub(crate) async fn execute_async_listener(
    handler: ErasedAsyncHandlerFn,
    event: EventType,
    event_name: &'static str,
    listener: AsyncListenerMeta,
    handler_timeout: Option<Duration>,
    async_limit: Option<Arc<tokio::sync::Semaphore>>,
) -> Option<ListenerFailure> {
    let mut retries_left = listener.subscription_policy.max_retries;
    let mut attempts = 0usize;
    loop {
        attempts += 1;

        #[cfg(feature = "metrics")]
        let _timer = TimerGuard::start("eventbus.handler.duration", event_name, listener.handler_name);

        let handler_future = CatchUnwindFuture::new(handler(Arc::clone(&event)));

        let run_attempt = async {
            match handler_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, handler_future).await {
                    Ok(Ok(inner)) => inner,
                    Ok(Err(panic_msg)) => Err(format!("handler panicked: {panic_msg}").into()),
                    Err(_) => Err(format!("handler timed out after {timeout:?}").into()),
                },
                None => match handler_future.await {
                    Ok(inner) => inner,
                    Err(panic_msg) => Err(format!("handler panicked: {panic_msg}").into()),
                },
            }
        };

        let result = if let Some(async_limit) = async_limit.as_ref() {
            match async_limit.acquire().await {
                Ok(_permit) => run_attempt.await,
                Err(_) => return None,
            }
        } else {
            run_attempt.await
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
                        handler_name: listener.handler_name,
                    });
                }

                retries_left -= 1;
                #[cfg(feature = "trace")]
                warn!(
                    event = event_name,
                    listener_id = listener.subscription_id.as_u64(),
                    handler_name = listener.handler_name,
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

pub(crate) async fn dispatch_slot(
    slot: &TypeSlot,
    event: &EventType,
    event_name: &'static str,
    dispatch_ctx: &DispatchContext<'_>,
) -> Vec<SubscriptionId> {
    let mut once_removed = Vec::new();

    // Async lane: schedule async handlers without waiting for sync gate.
    if dispatch_ctx.spawn_async_handlers && !slot.async_listeners.is_empty() {
        for listener in slot.async_listeners.iter() {
            if !should_fire_once(listener.once, listener.fired.as_ref()) {
                continue;
            }

            if listener.once {
                once_removed.push(listener.id);
            }

            if !spawn_async_listener(listener, Arc::clone(event), event_name, dispatch_ctx) && listener.once {
                once_removed.pop();
            }
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
            finalize_listener_failure(dispatch_ctx.bus, sync_listener_failed(listener, event_name, event, err.to_string())).await;
        }
    }

    once_removed
}

fn spawn_async_listener(listener: &AsyncListenerEntry, event: EventType, event_name: &'static str, dispatch_ctx: &DispatchContext<'_>) -> bool {
    let bus = dispatch_ctx.bus.clone();
    let handler = Arc::clone(&listener.handler);
    let async_limit = dispatch_ctx.bus.inner.async_limit.as_ref().map(Arc::clone);
    let meta = AsyncListenerMeta {
        subscription_id: listener.id,
        subscription_policy: listener.subscription_policy,
        handler_name: listener.name,
    };

    let task = async move {
        if let Some(failure) = execute_async_listener(handler, event, event_name, meta, bus.inner.handler_timeout, async_limit).await {
            finalize_listener_failure(&bus, failure).await;
        }
    };

    #[cfg(feature = "trace")]
    let task = task.instrument(dispatch_ctx.parent_span.clone());

    dispatch_ctx.bus.inner.tracker.spawn_tracked(task)
}

pub(crate) async fn dispatch_with_snapshot(
    snapshot: &RegistrySnapshot,
    slot: Option<&Arc<TypeSlot>>,
    event: EventType,
    event_name: &'static str,
    dispatch_ctx: &DispatchContext<'_>,
) -> Result<Vec<SubscriptionId>, EventBusError> {
    #[cfg(feature = "metrics")]
    counter!("eventbus.publish", "event" => event_name).increment(1);

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
                ErasedMiddleware::Async(f) => match AssertUnwindSafe(f(event_name, Arc::clone(&event))).catch_unwind().await {
                    Ok(decision) => decision,
                    Err(panic_payload) => return Err(middleware_panic_error(panic_payload)),
                },
                ErasedMiddleware::Sync(f) => match catch_unwind(AssertUnwindSafe(|| f(event_name, event.as_ref()))) {
                    Ok(decision) => decision,
                    Err(panic_payload) => return Err(middleware_panic_error(panic_payload)),
                },
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
                TypedMiddlewareEntry::Async(mw) => match AssertUnwindSafe(mw(event_name, Arc::clone(&event))).catch_unwind().await {
                    Ok(decision) => decision,
                    Err(panic_payload) => return Err(middleware_panic_error(panic_payload)),
                },
                TypedMiddlewareEntry::Sync(mw) => match catch_unwind(AssertUnwindSafe(|| mw(event_name, event.as_ref()))) {
                    Ok(decision) => decision,
                    Err(panic_payload) => return Err(middleware_panic_error(panic_payload)),
                },
            };
            if let MiddlewareDecision::Reject(reason) = decision {
                return Err(EventBusError::MiddlewareRejected(reason));
            }
        }
    }

    Ok(dispatch_slot(slot.as_ref(), &event, event_name, dispatch_ctx).await)
}

pub(crate) async fn dispatch_sync_only_with_snapshot(
    snapshot: &RegistrySnapshot,
    slot: Option<&Arc<TypeSlot>>,
    event: &EventType,
    event_name: &'static str,
    dispatch_ctx: &DispatchContext<'_>,
) -> Result<Vec<SubscriptionId>, EventBusError> {
    #[cfg(feature = "metrics")]
    counter!("eventbus.publish", "event" => event_name).increment(1);

    let event_any = event.as_ref() as &(dyn Any + Send + Sync);

    if snapshot.global_middlewares.is_empty() {
        let Some(slot) = slot else {
            return Ok(Vec::new());
        };
        if slot.middlewares.is_empty() && slot.sync_listeners.is_empty() {
            return Ok(Vec::new());
        }
    } else {
        for (_id, mw) in snapshot.global_middlewares.iter() {
            let decision = match mw {
                ErasedMiddleware::Sync(f) => match catch_unwind(AssertUnwindSafe(|| f(event_name, event_any))) {
                    Ok(decision) => decision,
                    Err(panic_payload) => return Err(middleware_panic_error(panic_payload)),
                },
                ErasedMiddleware::Async(_) => {
                    debug_assert!(false, "sync-only path entered with async global middleware");
                    return Err(EventBusError::MiddlewareRejected(
                        "internal dispatch invariant violated: async global middleware in sync-only path".to_string(),
                    ));
                }
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
                TypedMiddlewareEntry::Sync(mw) => match catch_unwind(AssertUnwindSafe(|| mw(event_name, event_any))) {
                    Ok(decision) => decision,
                    Err(panic_payload) => return Err(middleware_panic_error(panic_payload)),
                },
                TypedMiddlewareEntry::Async(_) => {
                    debug_assert!(false, "sync-only path entered with async typed middleware");
                    return Err(EventBusError::MiddlewareRejected(
                        "internal dispatch invariant violated: async typed middleware in sync-only path".to_string(),
                    ));
                }
            };
            if let MiddlewareDecision::Reject(reason) = decision {
                return Err(EventBusError::MiddlewareRejected(reason));
            }
        }
    }

    if slot.sync_listeners.is_empty() {
        return Ok(Vec::new());
    }

    let mut once_removed = Vec::new();
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

        let result = catch_unwind(AssertUnwindSafe(|| (listener.handler)(event_any)))
            .unwrap_or_else(|panic_payload| Err(extract_panic_message(panic_payload).into()));

        if let Err(err) = result {
            finalize_listener_failure(
                dispatch_ctx.bus,
                ListenerFailure {
                    event_name,
                    subscription_id: listener.id,
                    attempts: 1,
                    error: err.to_string(),
                    dead_letter: listener.subscription_policy.dead_letter,
                    event: Arc::clone(event),
                    handler_name: listener.name,
                },
            )
            .await;
        }
    }

    Ok(once_removed)
}

pub(crate) fn dead_letter_from_failure(failure: &ListenerFailure) -> Option<DeadLetter> {
    #[cfg(feature = "trace")]
    error!(
        event = failure.event_name,
        listener_id = failure.subscription_id.as_u64(),
        handler_name = failure.handler_name,
        attempts = failure.attempts,
        error = %failure.error,
        "handler.failed"
    );

    #[cfg(feature = "metrics")]
    if let Some(name) = failure.handler_name {
        counter!("eventbus.handler.error", "event" => failure.event_name, "listener" => name).increment(1);
    } else {
        counter!("eventbus.handler.error", "event" => failure.event_name).increment(1);
    }

    let dead_letter_type_id = TypeId::of::<DeadLetter>();
    if failure.dead_letter && (*failure.event).type_id() != dead_letter_type_id {
        #[cfg(feature = "metrics")]
        if let Some(name) = failure.handler_name {
            counter!("eventbus.dead_letter", "event" => failure.event_name, "handler" => name).increment(1);
        } else {
            counter!("eventbus.dead_letter", "event" => failure.event_name).increment(1);
        }

        Some(DeadLetter {
            event_name: failure.event_name,
            subscription_id: failure.subscription_id,
            attempts: failure.attempts,
            error: failure.error.clone(),
            event: failure.event.clone(),
            failed_at: std::time::SystemTime::now(),
            handler_name: failure.handler_name,
        })
    } else {
        None
    }
}
