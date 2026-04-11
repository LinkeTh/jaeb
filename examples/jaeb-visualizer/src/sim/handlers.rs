use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::types::ListenerConfig;
use crate::sim::SimEvent;

#[derive(Clone, Debug)]
pub struct SimEnvelopeEvent {
    pub seq: u64,
    pub event_type_idx: usize,
}

pub type ListenerLookup = Arc<HashMap<String, usize>>;

pub struct MockAsyncHandler {
    pub listener_idx: usize,
    pub cfg: ListenerConfig,
    pub tx: mpsc::UnboundedSender<SimEvent>,
}

impl jaeb::EventHandler<SimEnvelopeEvent> for MockAsyncHandler {
    fn handle(&self, event: &SimEnvelopeEvent) -> impl std::future::Future<Output = jaeb::HandlerResult> + Send {
        let tx = self.tx.clone();
        let listener_idx = self.listener_idx;
        let cfg = self.cfg.clone();
        let seq = event.seq;
        let event_type_idx = event.event_type_idx;
        async move {
            if event_type_idx != cfg.event_type_idx as usize {
                return Ok(());
            }

            let start = Instant::now();
            let _ = tx.send(SimEvent::HandlerStarted {
                listener: cfg.name.clone(),
                listener_idx,
                event_type_idx,
                seq,
                at: start,
            });

            tokio::time::sleep(Duration::from_millis(cfg.processing_ms)).await;
            if rand::random::<f64>() < cfg.failure_rate {
                let _ = tx.send(SimEvent::HandlerFailed {
                    listener: cfg.name,
                    listener_idx,
                    event_type_idx,
                    seq,
                    at: Instant::now(),
                });
                return Err("simulated failure".into());
            }

            let elapsed = start.elapsed().as_millis() as u64;
            let _ = tx.send(SimEvent::HandlerCompleted {
                listener: cfg.name,
                listener_idx,
                event_type_idx,
                seq,
                duration_ms: elapsed,
                at: Instant::now(),
            });
            Ok(())
        }
    }

    fn name(&self) -> Option<&'static str> {
        None
    }
}

pub struct MockSyncHandler {
    pub listener_idx: usize,
    pub cfg: ListenerConfig,
    pub tx: mpsc::UnboundedSender<SimEvent>,
}

impl jaeb::SyncEventHandler<SimEnvelopeEvent> for MockSyncHandler {
    fn handle(&self, event: &SimEnvelopeEvent) -> jaeb::HandlerResult {
        if event.event_type_idx != self.cfg.event_type_idx as usize {
            return Ok(());
        }

        let start = Instant::now();
        let _ = self.tx.send(SimEvent::HandlerStarted {
            listener: self.cfg.name.clone(),
            listener_idx: self.listener_idx,
            event_type_idx: event.event_type_idx,
            seq: event.seq,
            at: start,
        });

        std::thread::sleep(Duration::from_millis(self.cfg.processing_ms));
        if rand::random::<f64>() < self.cfg.failure_rate {
            let _ = self.tx.send(SimEvent::HandlerFailed {
                listener: self.cfg.name.clone(),
                listener_idx: self.listener_idx,
                event_type_idx: event.event_type_idx,
                seq: event.seq,
                at: Instant::now(),
            });
            return Err("simulated failure".into());
        }

        let elapsed = start.elapsed().as_millis() as u64;
        let _ = self.tx.send(SimEvent::HandlerCompleted {
            listener: self.cfg.name.clone(),
            listener_idx: self.listener_idx,
            event_type_idx: event.event_type_idx,
            seq: event.seq,
            duration_ms: elapsed,
            at: Instant::now(),
        });
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        None
    }
}

pub struct DeadLetterCollector {
    pub tx: mpsc::UnboundedSender<SimEvent>,
    pub listener_lookup: ListenerLookup,
}

impl jaeb::SyncEventHandler<jaeb::DeadLetter> for DeadLetterCollector {
    fn handle(&self, dl: &jaeb::DeadLetter) -> jaeb::HandlerResult {
        let envelope = dl.event.downcast_ref::<SimEnvelopeEvent>();
        let event_type_idx = envelope.map(|e| e.event_type_idx).unwrap_or(0);
        let seq = envelope.map(|e| e.seq).unwrap_or(0);

        let listener_name = dl.handler_name.unwrap_or("unknown").to_string();
        let listener_idx = self.listener_lookup.get(&listener_name).copied();
        let _ = self.tx.send(SimEvent::DeadLetterReceived {
            listener: listener_name,
            listener_idx,
            event_type_idx,
            seq,
            error: dl.error.clone(),
            at: Instant::now(),
        });
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("dead_letter_collector")
    }
}
