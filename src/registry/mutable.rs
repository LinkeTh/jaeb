use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};

use super::tracker::AsyncTaskTracker;
use super::types::{
    AsyncListenerEntry, ErasedMiddleware, ListenerEntry, ListenerKind, RegistrySnapshot, SyncListenerEntry, TypeSlot, TypedMiddlewareEntry,
    TypedMiddlewareSlot,
};
use super::worker::AsyncSlotWorker;
use crate::types::{HandlerInfo, SubscriptionId};

struct MutableTypeSlot {
    event_name: &'static str,
    listeners: Vec<ListenerEntry>,
    middlewares: Vec<TypedMiddlewareSlot>,
    sync_gate: Arc<Mutex<()>>,
    async_semaphore: Option<Arc<Semaphore>>,
    worker: Option<AsyncSlotWorker>,
}

impl MutableTypeSlot {
    fn to_snapshot_slot(&self) -> Arc<TypeSlot> {
        let mut sync_listeners: Vec<SyncListenerEntry> = Vec::new();
        let mut async_listeners: Vec<AsyncListenerEntry> = Vec::new();
        let mut has_async_middleware = false;
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

        for middleware in &self.middlewares {
            if matches!(middleware.middleware, TypedMiddlewareEntry::Async(_)) {
                has_async_middleware = true;
                break;
            }
        }

        Arc::new(TypeSlot {
            sync_listeners: sync_listeners.into(),
            async_listeners: async_listeners.into(),
            middlewares: self.middlewares.clone().into(),
            has_async_middleware,
            sync_gate: Arc::clone(&self.sync_gate),
            async_semaphore: self.async_semaphore.as_ref().map(Arc::clone),
            worker: self.worker.clone(),
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
    tracker: Arc<AsyncTaskTracker>,
}

impl MutableRegistry {
    pub(crate) fn new(max_concurrent_async: Option<usize>, tracker: Arc<AsyncTaskTracker>) -> Self {
        Self {
            slots: HashMap::new(),
            global_middlewares: Vec::new(),
            index: HashMap::new(),
            type_names: HashMap::new(),
            max_concurrent_async,
            tracker,
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
            worker: None,
        })
    }

    pub(crate) fn add_listener(&mut self, event_type: TypeId, event_name: &'static str, listener: ListenerEntry) {
        let is_async = matches!(listener.kind, ListenerKind::Async(_));
        let tracker = Arc::clone(&self.tracker);
        let slot = self.ensure_slot(event_type, event_name);
        slot.listeners.push(listener.clone());
        // Lazily create a persistent worker when the first async listener arrives.
        if is_async && slot.worker.is_none() {
            slot.worker = Some(AsyncSlotWorker::spawn(tracker));
        }
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
        let global_has_async_middleware = self
            .global_middlewares
            .iter()
            .any(|(_, middleware)| matches!(middleware, ErasedMiddleware::Async(_)));
        RegistrySnapshot {
            by_type,
            global_middlewares: self.global_middlewares.clone().into(),
            global_has_async_middleware,
        }
    }

    pub(crate) fn stats(
        &self,
        in_flight_async: usize,
        queue_capacity: usize,
        publish_permits_available: usize,
        shutdown_called: bool,
    ) -> crate::types::BusStats {
        let mut subscriptions_by_event: HashMap<&'static str, Vec<HandlerInfo>> = HashMap::new();
        let mut total_subscriptions = 0usize;
        let mut registered_event_types = Vec::new();

        for (type_id, slot) in &self.slots {
            if slot.listeners.is_empty() {
                continue;
            }
            let event_name = self.type_names.get(type_id).copied().unwrap_or(slot.event_name);
            registered_event_types.push(event_name);
            let infos: Vec<HandlerInfo> = slot
                .listeners
                .iter()
                .map(|l| HandlerInfo {
                    subscription_id: l.id,
                    name: l.name,
                })
                .collect();
            total_subscriptions += infos.len();
            subscriptions_by_event.insert(event_name, infos);
        }

        registered_event_types.sort_unstable();

        let publish_in_flight = queue_capacity.saturating_sub(publish_permits_available);

        crate::types::BusStats {
            total_subscriptions,
            subscriptions_by_event,
            registered_event_types,
            queue_capacity,
            publish_permits_available,
            publish_in_flight,
            in_flight_async,
            shutdown_called,
        }
    }
}
