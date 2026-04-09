// SPDX-License-Identifier: MIT
use std::fmt;

use crate::bus::EventBus;
use crate::error::EventBusError;
use crate::types::SubscriptionId;

#[derive(Clone)]
#[must_use = "dropping the Subscription leaves the listener registered; call .unsubscribe() or store the handle"]
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

    pub async fn unsubscribe(self) -> Result<bool, EventBusError> {
        self.bus.unsubscribe(self.id).await
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription").field("id", &self.id).finish()
    }
}
