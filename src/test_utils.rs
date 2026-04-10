//! Test utilities for asserting on events published through an [`EventBus`].
//!
//! Enable the `test-utils` feature to use this module:
//!
//! ```toml
//! [dev-dependencies]
//! jaeb = { version = "0.3", features = ["test-utils"] }
//! ```
//!
//! # Overview
//!
//! [`TestBus`] wraps a regular [`EventBus`] and adds per-type event capture
//! buffers. Call [`capture`](TestBus::capture) for each event type you want to
//! observe, then publish events normally and use [`published`](TestBus::published),
//! [`assert_count`](TestBus::assert_count), or [`assert_empty`](TestBus::assert_empty)
//! to verify behaviour.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::bus::{EventBus, EventBusBuilder};
use crate::error::EventBusError;
use crate::handler::SyncEventHandler;
use crate::types::{Event, FailurePolicy};

type AnyBuffer = Arc<Mutex<Vec<Box<dyn Any + Send + Sync>>>>;

/// A test-oriented wrapper around [`EventBus`] with built-in event capture.
pub struct TestBus {
    bus: EventBus,
    /// Per-type capture buffers, keyed by `TypeId`.
    buffers: Arc<Mutex<HashMap<TypeId, AnyBuffer>>>,
}

impl TestBus {
    /// Create a `TestBus` with default settings.
    pub fn new() -> Result<Self, EventBusError> {
        Ok(Self {
            bus: EventBus::new(256)?,
            buffers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Return a builder for fine-grained bus configuration.
    ///
    /// Call [`TestBusBuilder::build`] to finalize.
    pub fn builder() -> TestBusBuilder {
        TestBusBuilder { inner: EventBus::builder() }
    }

    /// Access the underlying [`EventBus`].
    pub fn inner(&self) -> &EventBus {
        &self.bus
    }

    /// Start capturing events of type `E`.
    ///
    /// This subscribes an internal sync handler that stores every published
    /// event in a buffer. Must be called **before** publishing events you
    /// wish to capture.
    ///
    /// Calling `capture` multiple times for the same `E` is idempotent — only
    /// the first call registers a handler.
    pub async fn capture<E>(&self) -> Result<(), EventBusError>
    where
        E: Event + Clone + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<E>();
        {
            let buffers = self.buffers.lock().expect("TestBus buffers lock poisoned");
            if buffers.contains_key(&type_id) {
                return Ok(()); // Already capturing this type.
            }
        }

        let buffer: AnyBuffer = Arc::new(Mutex::new(Vec::new()));
        {
            let mut buffers = self.buffers.lock().expect("TestBus buffers lock poisoned");
            buffers.insert(type_id, Arc::clone(&buffer));
        }

        struct CaptureHandler<E: Clone + Send + Sync + 'static> {
            buffer: AnyBuffer,
            _marker: std::marker::PhantomData<E>,
        }

        impl<E: Event + Clone + Send + Sync + 'static> SyncEventHandler<E> for CaptureHandler<E> {
            fn handle(&self, event: &E) -> crate::error::HandlerResult {
                let cloned: Box<dyn Any + Send + Sync> = Box::new(event.clone());
                self.buffer.lock().expect("capture buffer lock poisoned").push(cloned);
                Ok(())
            }
        }

        let handler = CaptureHandler::<E> {
            buffer,
            _marker: std::marker::PhantomData,
        };

        // Subscribe with dead_letter = false to avoid noise in tests.
        // The subscription handle is intentionally dropped — the capture
        // handler should remain registered for the lifetime of the TestBus.
        let policy = crate::NoRetryPolicy::default().with_dead_letter(false);
        let _sub = self.bus.subscribe_with_policy::<E, _, crate::handler::SyncMode>(handler, policy).await?;

        Ok(())
    }

    /// Return all captured events of type `E` (cloned from the buffer).
    ///
    /// Returns an empty `Vec` if `capture::<E>()` was never called or no
    /// events of that type have been published.
    pub fn published<E>(&self) -> Vec<E>
    where
        E: Event + Clone + 'static,
    {
        let buffers = self.buffers.lock().expect("TestBus buffers lock poisoned");
        let Some(buffer) = buffers.get(&TypeId::of::<E>()) else {
            return Vec::new();
        };
        let guard = buffer.lock().expect("capture buffer lock poisoned");
        guard.iter().filter_map(|any| any.downcast_ref::<E>().cloned()).collect()
    }

    /// Assert that exactly `expected` events of type `E` were captured.
    ///
    /// # Panics
    ///
    /// Panics with a descriptive message if the count does not match.
    pub fn assert_count<E>(&self, expected: usize)
    where
        E: Event + Clone + 'static,
    {
        let actual = self.published::<E>().len();
        assert_eq!(
            actual,
            expected,
            "TestBus::assert_count<{}>: expected {} events, got {}",
            std::any::type_name::<E>(),
            expected,
            actual,
        );
    }

    /// Assert that no events of type `E` were captured.
    ///
    /// # Panics
    ///
    /// Panics if any events of type `E` have been captured.
    pub fn assert_empty<E>(&self)
    where
        E: Event + Clone + 'static,
    {
        self.assert_count::<E>(0);
    }

    /// Shut down the underlying event bus.
    pub async fn shutdown(&self) -> Result<(), EventBusError> {
        self.bus.shutdown().await
    }
}

/// Builder for [`TestBus`], wrapping [`EventBusBuilder`].
pub struct TestBusBuilder {
    inner: EventBusBuilder,
}

impl TestBusBuilder {
    /// See [`EventBusBuilder::buffer_size`].
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.inner = self.inner.buffer_size(size);
        self
    }

    /// See [`EventBusBuilder::handler_timeout`].
    pub fn handler_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.inner = self.inner.handler_timeout(timeout);
        self
    }

    /// See [`EventBusBuilder::max_concurrent_async`].
    pub fn max_concurrent_async(mut self, max: usize) -> Self {
        self.inner = self.inner.max_concurrent_async(max);
        self
    }

    /// See [`EventBusBuilder::shutdown_timeout`].
    pub fn shutdown_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.inner = self.inner.shutdown_timeout(timeout);
        self
    }

    /// See [`EventBusBuilder::default_failure_policy`].
    pub fn default_failure_policy(mut self, policy: FailurePolicy) -> Self {
        self.inner = self.inner.default_failure_policy(policy);
        self
    }

    /// Build and return the [`TestBus`].
    pub fn build(self) -> Result<TestBus, EventBusError> {
        Ok(TestBus {
            bus: self.inner.build()?,
            buffers: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}
