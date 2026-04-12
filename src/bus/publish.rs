use std::any::{Any, TypeId};
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[cfg(feature = "metrics")]
use metrics::counter;

use crate::error::EventBusError;
use crate::registry::{DispatchContext, RegistrySnapshot, TypeSlot, dispatch_slot, dispatch_sync_only_with_snapshot, dispatch_with_snapshot};
use crate::types::Event;

use super::EventBus;

impl EventBus {
    pub(super) async fn publish_erased(
        &self,
        snapshot: &RegistrySnapshot,
        slot: Option<&Arc<TypeSlot>>,
        event: Arc<dyn Any + Send + Sync>,
        event_name: &'static str,
        dispatch_ctx: &DispatchContext<'_>,
    ) -> Result<(), EventBusError> {
        // Fast path: no middleware at all — skip dispatch_with_snapshot indirection.
        let once_removed = if snapshot.global_middlewares.is_empty() && slot.is_none_or(|s| s.middlewares.is_empty()) {
            #[cfg(feature = "metrics")]
            counter!("eventbus.publish", "event" => event_name).increment(1);

            match slot {
                None => Vec::new(),
                Some(s) => dispatch_slot(s.as_ref(), &event, event_name, dispatch_ctx).await,
            }
        } else {
            dispatch_with_snapshot(snapshot, slot, event, event_name, dispatch_ctx).await?
        };

        if !once_removed.is_empty() {
            let mut registry = self.inner.registry.lock().await;
            for subscription_id in once_removed {
                registry.remove_once(subscription_id);
            }
            self.refresh_snapshot_locked(&registry).await;
        }

        Ok(())
    }

    async fn publish_sync_only<E>(
        &self,
        snapshot: &RegistrySnapshot,
        slot: Option<&Arc<TypeSlot>>,
        event: E,
        event_name: &'static str,
    ) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        let once_removed = dispatch_sync_only_with_snapshot(snapshot, slot, &event, event_name, &self.inner.notify_tx).await?;

        if !once_removed.is_empty() {
            let mut registry = self.inner.registry.lock().await;
            for subscription_id in once_removed {
                registry.remove_once(subscription_id);
            }
            self.refresh_snapshot_locked(&registry).await;
        }

        Ok(())
    }

    /// Publish an event to all registered listeners.
    ///
    /// Dispatch behaviour depends on handler type:
    /// - **Sync handlers** run inline; `publish` waits for each one to return.
    /// - **Async handlers** are spawned as separate tasks; `publish` returns
    ///   once all tasks have been *spawned*, not necessarily *completed*.
    ///
    /// If the internal channel buffer is full this method waits asynchronously
    /// until capacity is available. Use [`try_publish`](Self::try_publish) for
    /// a non-blocking alternative.
    ///
    /// Events with no registered listeners (and no global middleware) are
    /// silently dropped without allocating.
    ///
    /// # Errors
    ///
    /// - [`EventBusError::Stopped`] — the bus has been shut down.
    /// - [`EventBusError::MiddlewareRejected`] — a middleware rejected the event.
    pub async fn publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        let snapshot = self.inner.snapshot.load();
        let event_type = TypeId::of::<E>();
        let slot = snapshot.by_type.get(&event_type);
        if slot.is_none() && snapshot.global_middlewares.is_empty() {
            return Ok(());
        }

        let _permit = match self.inner.publish_permits.try_acquire() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => self.inner.publish_permits.acquire().await.map_err(|_| EventBusError::Stopped)?,
            Err(tokio::sync::TryAcquireError::Closed) => return Err(EventBusError::Stopped),
        };
        let sync_only =
            !snapshot.global_has_async_middleware && slot.is_none_or(|slot| slot.async_listeners.is_empty() && !slot.has_async_middleware);

        if sync_only {
            self.publish_sync_only(snapshot.as_ref(), slot, event, std::any::type_name::<E>()).await
        } else {
            let dispatch_ctx = self.inner.full_dispatch_context();
            self.publish_erased(snapshot.as_ref(), slot, Arc::new(event), std::any::type_name::<E>(), &dispatch_ctx)
                .await
        }
    }

    /// Attempt to publish an event without waiting for buffer capacity.
    ///
    /// If there is room in the internal channel the event is enqueued and
    /// dispatched in a background task; otherwise
    /// [`EventBusError::ChannelFull`] is returned immediately.
    ///
    /// Because dispatch happens in a background task, errors from individual
    /// listeners are logged via `tracing` but are **not** propagated to the
    /// caller. Use [`publish`](Self::publish) if you need to observe per-listener
    /// errors or middleware rejections synchronously.
    ///
    /// # Errors
    ///
    /// - [`EventBusError::Stopped`] — the bus has been shut down.
    /// - [`EventBusError::ChannelFull`] — no buffer space is available.
    pub fn try_publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        let snapshot = self.inner.snapshot.load_full();
        let event_type = TypeId::of::<E>();
        let slot = snapshot.by_type.get(&event_type);
        if slot.is_none() && snapshot.global_middlewares.is_empty() {
            return Ok(());
        }
        let sync_only =
            !snapshot.global_has_async_middleware && slot.is_none_or(|slot| slot.async_listeners.is_empty() && !slot.has_async_middleware);

        let Ok(permit) = Arc::clone(&self.inner.publish_permits).try_acquire_owned() else {
            return Err(EventBusError::ChannelFull);
        };
        let slot = slot.cloned();

        let bus = self.clone();
        // Capture the caller's span *before* spawning so the dispatched task
        // inherits the correct trace context (tokio::spawn breaks the span chain).
        #[cfg(feature = "trace")]
        let caller_span = tracing::Span::current();
        tokio::spawn(async move {
            let _keep = permit;
            let slot = slot.as_ref();
            if sync_only {
                if let Err(_err) = bus.publish_sync_only(snapshot.as_ref(), slot, event, std::any::type_name::<E>()).await {
                    #[cfg(feature = "trace")]
                    tracing::error!(error = %_err, "event_bus.try_publish.dispatch_failed");
                }
            } else {
                let dispatch_ctx = DispatchContext {
                    tracker: &bus.inner.tracker,
                    notify_tx: &bus.inner.notify_tx,
                    handler_timeout: bus.inner.handler_timeout,
                    spawn_async_handlers: true,
                    #[cfg(feature = "trace")]
                    parent_span: caller_span,
                };
                if let Err(_err) = bus
                    .publish_erased(snapshot.as_ref(), slot, Arc::new(event), std::any::type_name::<E>(), &dispatch_ctx)
                    .await
                {
                    #[cfg(feature = "trace")]
                    tracing::error!(error = %_err, "event_bus.try_publish.dispatch_failed");
                }
            }
        });
        Ok(())
    }
}
