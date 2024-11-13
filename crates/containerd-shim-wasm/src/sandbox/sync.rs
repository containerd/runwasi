use std::cell::OnceCell;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A cell where we can wait (with timeout) for
/// a value to be set
pub struct WaitableCell<T> {
    inner: Arc<WaitableCellImpl<T>>,
}

struct WaitableCellImpl<T> {
    // Ideally we would just use a OnceLock, but it doesn't
    // have the `wait` and `wait_timeout` methods, so we use
    // a Condvar + Mutex pair instead.
    // We can't guard the OnceCell **inside** the Mutex as
    // that would produce ownership problems with returning
    // `&T`. This is because the mutex doesn't know that we
    // won't mutate the OnceCell once it's set.
    mutex: Mutex<()>,
    cvar: Condvar,
    cell: OnceCell<T>,
}

// this is safe because access to cell guarded by the mutex
unsafe impl<T> Send for WaitableCell<T> {}
unsafe impl<T> Sync for WaitableCell<T> {}

impl<T> Default for WaitableCell<T> {
    fn default() -> Self {
        Self {
            inner: Arc::new(WaitableCellImpl {
                mutex: Mutex::new(()),
                cvar: Condvar::new(),
                cell: OnceCell::new(),
            }),
        }
    }
}

impl<T> Clone for WaitableCell<T> {
    fn clone(&self) -> Self {
        let inner = self.inner.clone();
        Self { inner }
    }
}

impl<T> WaitableCell<T> {
    /// Creates an empty WaitableCell.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a value to the WaitableCell.
    /// This method has no effect if the WaitableCell already has a value.
    pub fn set(&self, val: impl Into<T>) -> Result<(), T> {
        let val = val.into();
        let _guard = self.inner.mutex.lock().unwrap();
        let res = self.inner.cell.set(val);
        self.inner.cvar.notify_all();
        res
    }

    /// If the `WaitableCell` is empty when this guard is dropped, the cell will be set to result of `f`.
    /// ```
    /// # use containerd_shim_wasm::sandbox::sync::WaitableCell;
    /// let cell = WaitableCell::<i32>::new();
    /// {
    ///     let _guard = cell.set_guard_with(|| 42);
    /// }
    /// assert_eq!(&42, cell.wait());
    /// ```
    ///
    /// The operation is a no-op if the cell conbtains a value before the guard is dropped.
    /// ```
    /// # use containerd_shim_wasm::sandbox::sync::WaitableCell;
    /// let cell = WaitableCell::<i32>::new();
    /// {
    ///     let _guard = cell.set_guard_with(|| 42);
    ///     let _ = cell.set(24);
    /// }
    /// assert_eq!(&24, cell.wait());
    /// ```
    ///
    /// The function `f` will always be called, regardless of whether the `WaitableCell` has a value or not.
    /// The `WaitableCell` is going to be set even in the case of an unwind. In this case, ff the function `f`
    /// panics it will cause an abort, so it's recommended to avoid any panics in `f`.
    pub fn set_guard_with<R: Into<T>>(&self, f: impl FnOnce() -> R) -> impl Drop {
        let cell = (*self).clone();
        WaitableCellSetGuard { f: Some(f), cell }
    }

    /// Wait for the WaitableCell to be set a value.
    pub fn wait(&self) -> &T {
        let value = self.wait_timeout(None);
        // safe because we waited with timeout `None`
        unsafe { value.unwrap_unchecked() }
    }

    /// Wait for the WaitableCell to be set a value, with timeout.
    /// Returns None if the timeout is reached with no value.
    pub fn wait_timeout(&self, timeout: impl Into<Option<Duration>>) -> Option<&T> {
        let timeout = timeout.into();
        let cvar = &self.inner.cvar;
        let guard = self.inner.mutex.lock().unwrap();
        let _guard = match timeout {
            None => cvar
                .wait_while(guard, |_| self.inner.cell.get().is_none())
                .unwrap(),
            Some(Duration::ZERO) => guard,
            Some(dur) => cvar
                .wait_timeout_while(guard, dur, |_| self.inner.cell.get().is_none())
                .map(|(guard, _)| guard)
                .unwrap(),
        };
        self.inner.cell.get()
    }
}

// This is the type returned by `WaitableCell::set_guard_with`.
// The public API has no visibility over this type, other than it implements `Drop`
// If the `WaitableCell` `cell`` is empty when this guard is dropped, it will set it's value with the result of `f`.
struct WaitableCellSetGuard<T, R: Into<T>, F: FnOnce() -> R> {
    f: Option<F>,
    cell: WaitableCell<T>,
}

impl<T, R: Into<T>, F: FnOnce() -> R> Drop for WaitableCellSetGuard<T, R, F> {
    fn drop(&mut self) {
        let _ = self.cell.set(self.f.take().unwrap()());
    }
}

#[cfg(test)]
mod test {
    use std::thread::{sleep, spawn};
    use std::time::Duration;

    use super::WaitableCell;

    #[test]
    fn basic() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(&42, cell.wait());
    }

    #[test]
    fn basic_timeout_zero() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(Some(&42), cell.wait_timeout(Duration::ZERO));
    }

    #[test]
    fn basic_timeout_1ms() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(Some(&42), cell.wait_timeout(Duration::from_secs(1)));
    }

    #[test]
    fn basic_timeout_none() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(Some(&42), cell.wait_timeout(None));
    }

    #[test]
    fn unset_timeout_zero() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(None, cell.wait_timeout(Duration::ZERO));
    }

    #[test]
    fn unset_timeout_1ms() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(None, cell.wait_timeout(Duration::from_millis(1)));
    }

    #[test]
    fn clone() {
        let cell = WaitableCell::<i32>::new();
        let cloned = cell.clone();
        let _ = cloned.set(42);
        assert_eq!(&42, cell.wait());
    }

    #[test]
    fn basic_threaded() {
        let cell = WaitableCell::<i32>::new();
        {
            let cell = cell.clone();
            spawn(move || {
                sleep(Duration::from_millis(1));
                let _ = cell.set(42);
            });
        }
        assert_eq!(&42, cell.wait());
    }

    #[test]
    fn basic_double_set() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(Ok(()), cell.set(42));
        assert_eq!(Err(24), cell.set(24));
    }

    #[test]
    fn guard() {
        let cell = WaitableCell::<i32>::new();
        {
            let _guard = cell.set_guard_with(|| 42);
        }
        assert_eq!(&42, cell.wait());
    }

    #[test]
    fn guard_no_op() {
        let cell = WaitableCell::<i32>::new();
        {
            let _guard = cell.set_guard_with(|| 42);
            let _ = cell.set(24);
        }
        assert_eq!(&24, cell.wait());
    }
}
