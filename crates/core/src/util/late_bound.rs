//! A late-bound, weak reference to a shared service.

use std::sync::{Arc, OnceLock, Weak};

/// A late-bound, weak reference to a `T` that is built after the values that
/// hold the reference.
///
/// The hub's agent host is built after the shared services every agent
/// receives, and each agent's tools are built inside a host-started agent, so
/// the services carry this handle and the host binds itself into it on
/// construction. It is weak because the agents hold it and the host holds the
/// agents; a strong reference would keep the host alive from inside its own
/// agents. Clones share one binding.
pub struct LateBound<T: ?Sized> {
    inner: Arc<OnceLock<Weak<T>>>,
}

impl<T: ?Sized> LateBound<T> {
    /// A handle that points at nothing until [`Self::bind`] is called.
    #[must_use]
    pub fn unbound() -> Self {
        Self {
            inner: Arc::new(OnceLock::new()),
        }
    }

    /// Point the handle at `target`. A handle binds once; later calls leave
    /// the first binding in place.
    pub fn bind(&self, target: Weak<T>) {
        self.inner.set(target).ok();
    }

    /// The target, when it is bound and still alive (the hub is not shutting
    /// down).
    #[must_use]
    pub fn get(&self) -> Option<Arc<T>> {
        self.inner.get().and_then(Weak::upgrade)
    }
}

impl<T: ?Sized> Clone for LateBound<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: ?Sized> Default for LateBound<T> {
    fn default() -> Self {
        Self::unbound()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Named {
        fn name(&self) -> &str;
    }

    struct Agent(&'static str);

    impl Named for Agent {
        fn name(&self) -> &str {
            self.0
        }
    }

    fn name_of(handle: &LateBound<dyn Named>) -> Option<String> {
        handle.get().map(|target| target.name().to_string())
    }

    #[test]
    fn an_unbound_handle_has_no_target() {
        let handle = LateBound::<dyn Named>::unbound();
        assert!(handle.get().is_none());
    }

    #[test]
    fn a_bound_handle_reads_through_and_forgets_a_dropped_target() {
        let target: Arc<dyn Named> = Arc::new(Agent("atlas"));
        let handle = LateBound::<dyn Named>::unbound();
        let clone = handle.clone();
        handle.bind(Arc::downgrade(&target));
        assert_eq!(name_of(&handle).as_deref(), Some("atlas"));
        assert_eq!(
            name_of(&clone).as_deref(),
            Some("atlas"),
            "clones share the binding"
        );

        drop(target);
        assert!(handle.get().is_none(), "the target is gone");
    }

    #[test]
    fn a_handle_binds_once() {
        let first: Arc<dyn Named> = Arc::new(Agent("atlas"));
        let second: Arc<dyn Named> = Arc::new(Agent("nova"));
        let handle = LateBound::<dyn Named>::unbound();
        handle.bind(Arc::downgrade(&first));
        handle.bind(Arc::downgrade(&second));
        assert_eq!(name_of(&handle).as_deref(), Some("atlas"));
    }
}
