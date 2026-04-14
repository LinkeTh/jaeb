use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::ops::Deref;

#[cfg(feature = "trace")]
use tracing::warn;

use crate::error::EventBusError;

/// A type-map container for handler dependencies resolved at bus build time.
///
/// `Deps` maps Rust types to values by [`TypeId`]. It is populated once via
/// [`EventBusBuilder::deps`](crate::EventBusBuilder::deps) and consulted during
/// [`build`](crate::EventBusBuilder::build) to inject dependencies into each
/// registered handler. After `build` returns, `Deps` is dropped; there is no
/// runtime overhead on the publish/dispatch hot path.
///
/// # Type uniqueness
///
/// Each type `T` can appear **at most once** in a `Deps` instance. If you need
/// multiple values of the same underlying type, wrap them in distinct newtypes:
///
/// ```rust
/// use jaeb::Deps;
///
/// struct ReadDb(String);
/// struct WriteDb(String);
///
/// let deps = Deps::new()
///     .insert(ReadDb("postgres://read".into()))
///     .insert(WriteDb("postgres://write".into()));
/// ```
///
/// # Examples
///
/// ```rust
/// use std::sync::Arc;
/// use jaeb::Deps;
///
/// struct Mailer;
/// struct DbPool;
///
/// let deps = Deps::new()
///     .insert(Arc::new(Mailer))
///     .insert(Arc::new(DbPool));
/// ```
#[derive(Default)]
pub struct Deps {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Deps {
    /// Create an empty `Deps` container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value of type `T`, replacing any previously inserted value of
    /// the same type. Returns `self` for chaining.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use jaeb::Deps;
    ///
    /// struct Config { timeout_secs: u64 }
    ///
    /// let deps = Deps::new().insert(Config { timeout_secs: 30 });
    /// ```
    pub fn insert<T: Send + Sync + 'static>(mut self, val: T) -> Self {
        let key = TypeId::of::<T>();
        #[cfg(feature = "trace")]
        if self.map.contains_key(&key) {
            warn!(type_name = type_name::<T>(), "overwriting previously inserted dependency");
        }
        self.map.insert(key, Box::new(val));
        self
    }

    /// Return a reference to the value of type `T`, or `None` if it was not
    /// inserted.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map.get(&TypeId::of::<T>()).and_then(|v| v.downcast_ref::<T>())
    }

    /// Return a reference to the value of type `T`.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::MissingDependency`] when no value of type `T`
    /// has been registered. The error message includes the type name for
    /// diagnostics.
    pub fn get_required<T: Send + Sync + 'static>(&self) -> Result<&T, EventBusError> {
        self.get::<T>()
            .ok_or_else(|| EventBusError::MissingDependency(type_name::<T>().to_owned()))
    }
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps").field("count", &self.map.len()).finish()
    }
}

/// Newtype wrapper used in `#[handler]` function signatures to declare a
/// handler dependency.
///
/// When the `macros` feature is enabled and a handler function accepts
/// parameters of the form `Dep(name): Dep<MyType>`, the generated descriptor
/// will resolve `MyType` from [`Deps`] at build time and inject it into the
/// handler.
///
/// You can also use `Dep<T>` in manual `HandlerDescriptor` implementations to
/// make the injection contract explicit:
///
/// ```rust
/// use jaeb::{Dep, Deps};
/// use std::sync::Arc;
///
/// struct Mailer;
///
/// fn uses_dep(dep: Dep<Arc<Mailer>>) {
///     let _mailer: &Arc<Mailer> = &*dep;
/// }
/// ```
pub struct Dep<T>(pub T);

impl<T> Deref for Dep<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Dep<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Dep").field(&self.0).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        struct Foo(u32);
        let deps = Deps::new().insert(Foo(42));
        assert_eq!(deps.get::<Foo>().expect("Foo present").0, 42);
    }

    #[test]
    fn get_missing_returns_none() {
        struct Bar;
        let deps = Deps::new();
        assert!(deps.get::<Bar>().is_none());
    }

    #[test]
    fn get_required_missing_returns_error() {
        #[derive(Debug)]
        struct Baz;
        let deps = Deps::new();
        match deps.get_required::<Baz>() {
            Err(EventBusError::MissingDependency(name)) => {
                assert!(name.contains("Baz"), "expected type name in error, got: {name}");
            }
            other => panic!("expected MissingDependency, got {other:?}"),
        }
    }

    #[test]
    fn insert_overwrites_same_type() {
        struct Num(u32);
        let deps = Deps::new().insert(Num(1)).insert(Num(2));
        assert_eq!(deps.get::<Num>().expect("Num present").0, 2);
    }

    #[test]
    fn multiple_types_independent() {
        struct A(u32);
        struct B(&'static str);
        let deps = Deps::new().insert(A(10)).insert(B("hello"));
        assert_eq!(deps.get::<A>().unwrap().0, 10);
        assert_eq!(deps.get::<B>().unwrap().0, "hello");
    }

    #[test]
    fn dep_deref() {
        let d = Dep(99u32);
        assert_eq!(*d, 99u32);
    }
}
