use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use super::types::{
    AsyncListenerEntry, DeadLetterSlot, ErasedMiddleware, ListenerEntry, ListenerKind, RegistrySnapshot, SyncListenerEntry, TypeSlot,
    TypedMiddlewareEntry, TypedMiddlewareSlot,
};
use crate::types::DeadLetter;
use crate::types::{HandlerInfo, SubscriptionId};
use tokio::sync::Mutex;

struct MutableTypeSlot {
    event_name: &'static str,
    listeners: Vec<ListenerEntry>,
    middlewares: Vec<TypedMiddlewareSlot>,
    sync_gate: Arc<Mutex<()>>,
}

impl MutableTypeSlot {
    fn to_snapshot_slot(&self) -> Arc<TypeSlot> {
        let mut sync_listeners: Vec<SyncListenerEntry> = Vec::new();
        let mut async_listeners: Vec<AsyncListenerEntry> = Vec::new();
        let mut has_async_middleware = false;
        for listener in &self.listeners {
            match &listener.kind {
                ListenerKind::Sync(_) => sync_listeners.push(SyncListenerEntry::from(listener)),
                ListenerKind::Async(_) => async_listeners.push(AsyncListenerEntry::from(listener)),
            }
        }
        sync_listeners.sort_by(|a, b| {
            b.subscription_policy
                .priority
                .cmp(&a.subscription_policy.priority)
                .then_with(|| a.registration_order.cmp(&b.registration_order))
        });
        async_listeners.sort_by_key(|a| a.registration_order);

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
    dead_letter_listeners: Vec<ListenerEntry>,
    dead_letter_sync_gate: Arc<Mutex<()>>,
    global_middlewares: Vec<(SubscriptionId, ErasedMiddleware)>,
    index: HashMap<SubscriptionId, IndexEntry>,
    type_names: HashMap<TypeId, &'static str>,
}

impl MutableRegistry {
    pub(crate) fn new() -> Self {
        Self {
            slots: HashMap::new(),
            dead_letter_listeners: Vec::new(),
            dead_letter_sync_gate: Arc::new(Mutex::new(())),
            global_middlewares: Vec::new(),
            index: HashMap::new(),
            type_names: HashMap::new(),
        }
    }

    fn ensure_slot(&mut self, event_type: TypeId, event_name: &'static str) -> &mut MutableTypeSlot {
        self.type_names.entry(event_type).or_insert(event_name);
        self.slots.entry(event_type).or_insert_with(|| MutableTypeSlot {
            event_name,
            listeners: Vec::new(),
            middlewares: Vec::new(),
            sync_gate: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) fn add_listener(&mut self, event_type: TypeId, event_name: &'static str, listener: ListenerEntry) {
        if event_type == TypeId::of::<DeadLetter>() {
            self.dead_letter_listeners.push(listener.clone());
            self.index.insert(listener.id, IndexEntry::Listener(event_type));
            return;
        }

        let slot = self.ensure_slot(event_type, event_name);
        slot.listeners.push(listener.clone());
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
        if *event_type == TypeId::of::<DeadLetter>() {
            self.dead_letter_listeners.retain(|l| l.id != subscription_id);
            self.index.remove(&subscription_id);
            return;
        }
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
                if event_type == TypeId::of::<DeadLetter>() {
                    let before = self.dead_letter_listeners.len();
                    self.dead_letter_listeners.retain(|l| l.id != subscription_id);
                    return before != self.dead_letter_listeners.len();
                }
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
        let dead_letter = if self.dead_letter_listeners.is_empty() {
            None
        } else {
            let mut listeners: Vec<SyncListenerEntry> = self.dead_letter_listeners.iter().map(SyncListenerEntry::from).collect();
            listeners.sort_by(|a, b| {
                b.subscription_policy
                    .priority
                    .cmp(&a.subscription_policy.priority)
                    .then_with(|| a.registration_order.cmp(&b.registration_order))
            });
            Some(Arc::new(DeadLetterSlot {
                listeners: listeners.into(),
                sync_gate: Arc::clone(&self.dead_letter_sync_gate),
            }))
        };
        let global_has_async_middleware = self
            .global_middlewares
            .iter()
            .any(|(_, middleware)| matches!(middleware, ErasedMiddleware::Async(_)));
        RegistrySnapshot {
            by_type,
            dead_letter,
            global_middlewares: self.global_middlewares.clone().into(),
            global_has_async_middleware,
        }
    }

    pub(crate) fn stats(&self, in_flight_async: usize, dispatches_in_flight: usize, shutdown_called: bool) -> crate::types::BusStats {
        let mut subscriptions_by_event: HashMap<&'static str, Vec<HandlerInfo>> = HashMap::new();
        let mut total_subscriptions = 0usize;
        let mut registered_event_types = Vec::new();

        if !self.dead_letter_listeners.is_empty() {
            let event_name = std::any::type_name::<DeadLetter>();
            registered_event_types.push(event_name);
            let infos: Vec<HandlerInfo> = self
                .dead_letter_listeners
                .iter()
                .map(|l| HandlerInfo {
                    subscription_id: l.id,
                    name: l.name,
                })
                .collect();
            total_subscriptions += infos.len();
            subscriptions_by_event.insert(event_name, infos);
        }

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

        crate::types::BusStats {
            total_subscriptions,
            subscriptions_by_event,
            registered_event_types,
            dispatches_in_flight,
            in_flight_async,
            shutdown_called,
        }
    }
}
