use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::sim::SimEvent;

/// Two compile-time event types for simulation.
#[derive(Clone, Debug)]
pub struct SimEventA {
    pub seq: u64,
}

#[derive(Clone, Debug)]
pub struct SimEventB {
    pub seq: u64,
}

/// Async mock handler for SimEventA.
pub struct MockAsyncHandlerA {
    pub name: String,
    pub processing_ms: u64,
    pub failure_rate: f64,
    pub tx: mpsc::UnboundedSender<SimEvent>,
}

impl jaeb::EventHandler<SimEventA> for MockAsyncHandlerA {
    fn handle(&self, event: &SimEventA) -> impl std::future::Future<Output = jaeb::HandlerResult> + Send {
        let name = self.name.clone();
        let tx = self.tx.clone();
        let processing_ms = self.processing_ms;
        let failure_rate = self.failure_rate;
        let seq = event.seq;
        async move {
            let start = Instant::now();
            let _ = tx.send(SimEvent::HandlerStarted {
                listener: name.clone(),
                event_type: 0,
                seq,
                at: start,
            });
            tokio::time::sleep(Duration::from_millis(processing_ms)).await;
            if rand::random::<f64>() < failure_rate {
                let _ = tx.send(SimEvent::HandlerFailed {
                    listener: name,
                    event_type: 0,
                    seq,
                    at: Instant::now(),
                });
                return Err("simulated failure".into());
            }
            let elapsed = start.elapsed().as_millis() as u64;
            let _ = tx.send(SimEvent::HandlerCompleted {
                listener: name,
                event_type: 0,
                seq,
                duration_ms: elapsed,
                at: Instant::now(),
            });
            Ok(())
        }
    }

    fn name(&self) -> Option<&'static str> {
        None // dynamic names not supported by trait (requires &'static str)
    }
}

/// Async mock handler for SimEventB.
pub struct MockAsyncHandlerB {
    pub name: String,
    pub processing_ms: u64,
    pub failure_rate: f64,
    pub tx: mpsc::UnboundedSender<SimEvent>,
}

impl jaeb::EventHandler<SimEventB> for MockAsyncHandlerB {
    fn handle(&self, event: &SimEventB) -> impl std::future::Future<Output = jaeb::HandlerResult> + Send {
        let name = self.name.clone();
        let tx = self.tx.clone();
        let processing_ms = self.processing_ms;
        let failure_rate = self.failure_rate;
        let seq = event.seq;
        async move {
            let start = Instant::now();
            let _ = tx.send(SimEvent::HandlerStarted {
                listener: name.clone(),
                event_type: 1,
                seq,
                at: start,
            });
            tokio::time::sleep(Duration::from_millis(processing_ms)).await;
            if rand::random::<f64>() < failure_rate {
                let _ = tx.send(SimEvent::HandlerFailed {
                    listener: name,
                    event_type: 1,
                    seq,
                    at: Instant::now(),
                });
                return Err("simulated failure".into());
            }
            let elapsed = start.elapsed().as_millis() as u64;
            let _ = tx.send(SimEvent::HandlerCompleted {
                listener: name,
                event_type: 1,
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

/// Sync mock handler for SimEventA.
pub struct MockSyncHandlerA {
    pub name: String,
    pub processing_ms: u64,
    pub failure_rate: f64,
    pub tx: mpsc::UnboundedSender<SimEvent>,
}

impl jaeb::SyncEventHandler<SimEventA> for MockSyncHandlerA {
    fn handle(&self, event: &SimEventA) -> jaeb::HandlerResult {
        let start = Instant::now();
        let _ = self.tx.send(SimEvent::HandlerStarted {
            listener: self.name.clone(),
            event_type: 0,
            seq: event.seq,
            at: start,
        });
        std::thread::sleep(Duration::from_millis(self.processing_ms));
        if rand::random::<f64>() < self.failure_rate {
            let _ = self.tx.send(SimEvent::HandlerFailed {
                listener: self.name.clone(),
                event_type: 0,
                seq: event.seq,
                at: Instant::now(),
            });
            return Err("simulated failure".into());
        }
        let elapsed = start.elapsed().as_millis() as u64;
        let _ = self.tx.send(SimEvent::HandlerCompleted {
            listener: self.name.clone(),
            event_type: 0,
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

/// Sync mock handler for SimEventB.
pub struct MockSyncHandlerB {
    pub name: String,
    pub processing_ms: u64,
    pub failure_rate: f64,
    pub tx: mpsc::UnboundedSender<SimEvent>,
}

impl jaeb::SyncEventHandler<SimEventB> for MockSyncHandlerB {
    fn handle(&self, event: &SimEventB) -> jaeb::HandlerResult {
        let start = Instant::now();
        let _ = self.tx.send(SimEvent::HandlerStarted {
            listener: self.name.clone(),
            event_type: 1,
            seq: event.seq,
            at: start,
        });
        std::thread::sleep(Duration::from_millis(self.processing_ms));
        if rand::random::<f64>() < self.failure_rate {
            let _ = self.tx.send(SimEvent::HandlerFailed {
                listener: self.name.clone(),
                event_type: 1,
                seq: event.seq,
                at: Instant::now(),
            });
            return Err("simulated failure".into());
        }
        let elapsed = start.elapsed().as_millis() as u64;
        let _ = self.tx.send(SimEvent::HandlerCompleted {
            listener: self.name.clone(),
            event_type: 1,
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

/// Dead letter listener that forwards dead letters to the SimEvent channel.
pub struct DeadLetterCollector {
    pub tx: mpsc::UnboundedSender<SimEvent>,
}

impl jaeb::SyncEventHandler<jaeb::DeadLetter> for DeadLetterCollector {
    fn handle(&self, dl: &jaeb::DeadLetter) -> jaeb::HandlerResult {
        let event_type = if dl.event_name.contains("SimEventA") { 0u8 } else { 1u8 };
        let seq = dl
            .event
            .downcast_ref::<SimEventA>()
            .map(|e| e.seq)
            .or_else(|| dl.event.downcast_ref::<SimEventB>().map(|e| e.seq))
            .unwrap_or(0);

        let _ = self.tx.send(SimEvent::DeadLetterReceived {
            listener: dl.handler_name.unwrap_or("unknown").to_string(),
            event_type,
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
