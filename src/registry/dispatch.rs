use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use futures_util::{
    FutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use tokio::sync::mpsc;

#[cfg(feature = "trace")]
use tracing::{Instrument, error, warn};

#[cfg(feature = "metrics")]
use metrics::counter;

use super::types::{
    ControlNotification, DispatchContext, ErasedAsyncHandlerFn, ErasedMiddleware, EventType, HandlerFuture, ListenerFailure, RegistrySnapshot,
    SyncListenerEntry, TypeSlot, TypedMiddlewareEntry,
};
use super::worker::{WorkItem, WorkItemData};
use crate::error::{EventBusError, HandlerResult};
use crate::middleware::MiddlewareDecision;
use crate::types::{DeadLetter, SubscriptionId, SubscriptionPolicy};

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

impl std::future::Future for CatchUnwindFuture {
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
    pub subscription_policy: SubscriptionPolicy,
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

pub(crate) async fn execute_async_listener(
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
        let _timer = TimerGuard::start("eventbus.handler.duration", event_name, listener.handler_name);

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
        let handler_timeout = dispatch_ctx.handler_timeout;
        let semaphore = slot.async_semaphore.as_ref().map(Arc::clone);

        if slot.async_listeners.len() == 1 {
            let listener = &slot.async_listeners[0];
            if should_fire_once(listener.once, listener.fired.as_ref()) {
                if listener.once {
                    once_removed.push(listener.id);
                }

                let listener_meta = AsyncListenerMeta {
                    subscription_id: listener.id,
                    subscription_policy: listener.subscription_policy,
                    handler_name: listener.name,
                };

                // Fast path: use the persistent worker to avoid per-publish tokio::spawn().
                // Falls back to spawn for `once` listeners (need synchronous once_removed return).
                if !listener.once
                    && let Some(worker) = &slot.worker
                {
                    worker.send(WorkItem::from_data(
                        WorkItemData {
                            handler: Arc::clone(&listener.handler),
                            event: Arc::clone(event),
                            event_name,
                            meta: listener_meta,
                            handler_timeout,
                            notify_tx: dispatch_ctx.notify_tx.clone(),
                            concurrency_semaphore: semaphore,
                            #[cfg(feature = "trace")]
                            parent_span: dispatch_ctx.parent_span.clone(),
                        },
                        Arc::clone(dispatch_ctx.tracker),
                    ));
                } else {
                    let tracker = Arc::clone(dispatch_ctx.tracker);
                    let handler = Arc::clone(&listener.handler);
                    let event = Arc::clone(event);
                    let notify = dispatch_ctx.notify_tx.clone();
                    #[cfg(feature = "trace")]
                    let parent_span = dispatch_ctx.parent_span.clone();
                    tracker.spawn_tracked({
                        let fut = async move {
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
                        };
                        #[cfg(feature = "trace")]
                        let fut = fut.instrument(parent_span);
                        fut
                    });
                }
            }
        } else {
            let mut async_batch = FuturesUnordered::new();

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
                    handler_name: listener.name,
                };
                let handler = Arc::clone(&listener.handler);
                let event = Arc::clone(event);
                let semaphore = semaphore.as_ref().map(Arc::clone);
                async_batch.push(async move {
                    let task = async { execute_async_listener(handler, event, event_name, listener_meta, handler_timeout).await };

                    if let Some(semaphore) = semaphore {
                        if let Ok(_permit) = semaphore.acquire().await { task.await } else { None }
                    } else {
                        task.await
                    }
                });
            }

            if !async_batch.is_empty() {
                let notify = dispatch_ctx.notify_tx.clone();
                let tracker = Arc::clone(dispatch_ctx.tracker);
                #[cfg(feature = "trace")]
                let parent_span = dispatch_ctx.parent_span.clone();
                tracker.spawn_tracked({
                    let fut = async move {
                        while let Some(failure) = async_batch.next().await {
                            if let Some(failure) = failure {
                                let _ = notify.send(ControlNotification::Failure(failure));
                            }
                        }
                    };
                    #[cfg(feature = "trace")]
                    let fut = fut.instrument(parent_span);
                    fut
                });
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

pub(crate) async fn dispatch_sync_only_with_snapshot<E>(
    snapshot: &RegistrySnapshot,
    slot: Option<&Arc<TypeSlot>>,
    event: &E,
    event_name: &'static str,
    notify_tx: &mpsc::UnboundedSender<ControlNotification>,
) -> Result<Vec<SubscriptionId>, EventBusError>
where
    E: Any + Send + Sync + Clone + 'static,
{
    #[cfg(feature = "metrics")]
    counter!("eventbus.publish", "event" => event_name).increment(1);

    let event_any = event as &(dyn Any + Send + Sync);

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
    let mut failure_event: Option<EventType> = None;
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
            let event_payload = failure_event.get_or_insert_with(|| Arc::new(event.clone()) as EventType).clone();
            let _ = notify_tx.send(ControlNotification::Failure(ListenerFailure {
                event_name,
                subscription_id: listener.id,
                attempts: 1,
                error: err.to_string(),
                dead_letter: listener.subscription_policy.dead_letter,
                event: event_payload,
                handler_name: listener.name,
            }));
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

    let dead_letter_type = std::any::type_name::<DeadLetter>();
    if failure.dead_letter && failure.event_name != dead_letter_type {
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
