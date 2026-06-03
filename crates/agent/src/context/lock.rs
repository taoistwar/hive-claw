//! CategoryLock: wrapper around `Arc<RwLock<T>>` for per-category thread-safe storage.
//!
//! Provides a `Clone`-able handle to shared mutable state with separate
//! read and write accessors.

use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A cloneable, thread-safe lock wrapper for a value of type `T`.
///
/// Internally backed by `Arc<RwLock<T>>`. Cloning creates a new handle
/// pointing to the same underlying lock and data.
pub struct CategoryLock<T> {
    inner: Arc<RwLock<T>>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for CategoryLock<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner.read() {
            Ok(guard) => f.debug_tuple("CategoryLock").field(&*guard).finish(),
            Err(_) => f.debug_tuple("CategoryLock").field(&"<poisoned>").finish(),
        }
    }
}

impl<T> CategoryLock<T> {
    /// Create a new `CategoryLock` wrapping the given value.
    pub fn new(data: T) -> Self {
        Self {
            inner: Arc::new(RwLock::new(data)),
        }
    }

    /// Acquire a read guard.
    ///
    /// Returns an error if the lock is poisoned (a writer panicked while holding it).
    pub fn read(&self) -> Result<RwLockReadGuard<'_, T>, String> {
        self.inner
            .read()
            .map_err(|e| format!("Read lock poisoned: {e}"))
    }

    /// Acquire a write guard.
    ///
    /// Returns an error if the lock is poisoned (a writer panicked while holding it).
    pub fn write(&self) -> Result<RwLockWriteGuard<'_, T>, String> {
        self.inner
            .write()
            .map_err(|e| format!("Write lock poisoned: {e}"))
    }
}

impl<T> Clone for CategoryLock<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_lock_read_write() {
        let lock = CategoryLock::new(42i32);
        assert_eq!(*lock.read().unwrap(), 42);

        {
            let mut guard = lock.write().unwrap();
            *guard = 100;
        }

        assert_eq!(*lock.read().unwrap(), 100);
    }

    #[test]
    fn category_lock_clone_shares_state() {
        let lock1 = CategoryLock::new(vec![1, 2, 3]);
        let lock2 = lock1.clone();

        {
            let mut guard = lock2.write().unwrap();
            guard.push(4);
        }

        assert_eq!(lock1.read().unwrap().len(), 4);
    }
}
