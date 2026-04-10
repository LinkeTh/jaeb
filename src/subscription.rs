use std::fmt;

use tracing::trace;

use crate::bus::EventBus;
use crate::error::EventBusError;
use crate::types::SubscriptionId;

#[derive(Clone)]
#[must_use = "dropping the Subscription leaves the listener registered; call .unsubscribe() or .into_guard() or store the handle"]
pub struct Subscription {
    id: SubscriptionId,
    bus: EventBus,
}

impl Subscription {
    pub(crate) fn new(id: SubscriptionId, bus: EventBus) -> Self {
        Self { id, bus }
    }

    pub const fn id(&self) -> SubscriptionId {
        self.id
    }

    /// Remove this listener from the bus.
    ///
    /// Returns `Ok(true)` if the listener was found and removed, `Ok(false)` if
    /// it was already removed, or `Err` if the bus has shut down.
    pub async fn unsubscribe(self) -> Result<bool, EventBusError> {
        self.bus.unsubscribe(self.id).await
    }

    /// Convert this subscription into a guard that automatically unsubscribes
    /// when dropped.
    ///
    /// The guard sends a fire-and-forget unsubscribe message in its [`Drop`]
    /// impl, so no `.await` is needed. If the bus has already shut down the
    /// message is silently discarded.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::{EventBus, SyncEventHandler, HandlerResult};
    ///
    /// #[derive(Clone)]
    /// struct Evt;
    ///
    /// struct H;
    /// impl SyncEventHandler<Evt> for H {
    ///     fn handle(&self, _: &Evt) -> HandlerResult { Ok(()) }
    /// }
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64).expect("valid config");
    /// {
    ///     let _guard = bus.subscribe::<Evt, _, _>(H).await.unwrap().into_guard();
    ///     // listener is active inside this scope
    /// }
    /// // guard dropped → listener automatically unsubscribed
    /// bus.shutdown().await.unwrap();
    /// # }
    /// ```
    pub fn into_guard(self) -> SubscriptionGuard {
        SubscriptionGuard::new(self.id, self.bus)
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription").field("id", &self.id).finish()
    }
}

/// RAII guard that automatically unsubscribes a listener when dropped.
///
/// Created via [`Subscription::into_guard`]. The unsubscribe is fire-and-forget
/// (no acknowledgement is awaited), which makes it safe to use in synchronous
/// `Drop` without blocking the runtime.
///
/// Cloning a `SubscriptionGuard` is intentionally **not** supported — each
/// guard owns exactly one unsubscribe action.
#[must_use = "dropping the SubscriptionGuard immediately will unsubscribe the listener"]
pub struct SubscriptionGuard {
    /// `None` after the guard has been explicitly disarmed or the unsubscribe
    /// has already been sent (double-drop safety).
    inner: Option<GuardInner>,
}

struct GuardInner {
    subscription_id: SubscriptionId,
    bus: EventBus,
}

impl SubscriptionGuard {
    fn new(subscription_id: SubscriptionId, bus: EventBus) -> Self {
        Self {
            inner: Some(GuardInner { subscription_id, bus }),
        }
    }

    /// Return the subscription ID this guard manages.
    pub fn id(&self) -> Option<SubscriptionId> {
        self.inner.as_ref().map(|i| i.subscription_id)
    }

    /// Disarm the guard without unsubscribing.
    ///
    /// After calling this the listener remains registered and the guard's
    /// `Drop` will be a no-op.
    pub fn disarm(&mut self) {
        self.inner.take();
    }
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            trace!(subscription_id = inner.subscription_id.as_u64(), "subscription_guard.drop.unsubscribe");
            if inner.bus.try_unsubscribe_best_effort(inner.subscription_id) {
                return;
            }

            let bus = inner.bus;
            let subscription_id = inner.subscription_id;
            tokio::spawn(async move {
                let _ = bus.unsubscribe(subscription_id).await;
            });
        }
    }
}

impl fmt::Debug for SubscriptionGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionGuard")
            .field("id", &self.inner.as_ref().map(|i| i.subscription_id))
            .finish()
    }
}
