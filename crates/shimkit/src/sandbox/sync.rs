//! Synchronization primitives (e.g. `WaitableCell`) for the sandbox.

use std::cell::OnceCell;
use std::sync::Arc;

use tokio::sync::Notify;

/// A cell where we can wait (with timeout) for
/// a value to be set
pub struct WaitableCell<T> {
    inner: Arc<WaitableCellImpl<T>>,
}

struct WaitableCellImpl<T> {
    cell: OnceCell<T>,
    notify: Notify,
}

// this is safe because access to cell guarded by the mutex
unsafe impl<T> Send for WaitableCell<T> {}
unsafe impl<T> Sync for WaitableCell<T> {}

impl<T> Default for WaitableCell<T> {
    fn default() -> Self {
        Self {
            inner: Arc::new(WaitableCellImpl {
                cell: OnceCell::default(),
                notify: Notify::new(),
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
        self.inner.cell.set(val.into())?;
        self.inner.notify.notify_waiters();
        Ok(())
    }

    /// If the `WaitableCell` is empty when this guard is dropped, the cell will be set to result of `f`.
    /// ```
    /// # use shimkit::sandbox::sync::WaitableCell;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let cell = WaitableCell::<i32>::new();
    /// {
    ///     let _guard = cell.set_guard_with(|| 42);
    /// }
    /// assert_eq!(&42, cell.wait().await);
    /// # })
    /// ```
    ///
    /// The operation is a no-op if the cell conbtains a value before the guard is dropped.
    /// ```
    /// # use shimkit::sandbox::sync::WaitableCell;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let cell = WaitableCell::<i32>::new();
    /// {
    ///     let _guard = cell.set_guard_with(|| 42);
    ///     let _ = cell.set(24);
    /// }
    /// assert_eq!(&24, cell.wait().await);
    /// # })
    /// ```
    ///
    /// The function `f` will always be called, regardless of whether the `WaitableCell` has a value or not.
    /// The `WaitableCell` is going to be set even in the case of an unwind. In this case, ff the function `f`
    /// panics it will cause an abort, so it's recommended to avoid any panics in `f`.
    pub fn set_guard_with<R: Into<T>, F: FnOnce() -> R>(&self, f: F) -> impl Drop + use<F, T, R> {
        let cell = (*self).clone();
        WaitableCellSetGuard { f: Some(f), cell }
    }

    /// Wait for the WaitableCell to be set a value.
    pub async fn wait(&self) -> &T {
        let notified = self.inner.notify.notified();
        if let Some(val) = self.inner.cell.get() {
            return val;
        }
        notified.await;
        // safe because we've been notified, which can only happen
        // a call to self.inner.cell.set(..)
        unsafe { self.inner.cell.get().unwrap_unchecked() }
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
    use std::time::Duration;

    use futures::FutureExt;
    use tokio::time::{sleep, timeout};

    use super::WaitableCell;

    #[tokio::test]
    async fn basic() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test]
    async fn basic_timeout_zero() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(Some(&42), cell.wait().now_or_never());
    }

    #[tokio::test]
    async fn unset_timeout_zero() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(None, cell.wait().now_or_never());
    }

    #[tokio::test]
    async fn unset_timeout_1ms() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(
            None,
            timeout(Duration::from_millis(1), cell.wait()).await.ok()
        );
    }

    #[tokio::test]
    async fn clone() {
        let cell = WaitableCell::<i32>::new();
        let cloned = cell.clone();
        let _ = cloned.set(42);
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test]
    async fn basic_threaded() {
        let cell = WaitableCell::<i32>::new();
        tokio::spawn({
            let cell = cell.clone();
            async move {
                sleep(Duration::from_millis(1)).await;
                let _ = cell.set(42);
            }
        });
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test]
    async fn basic_double_set() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(Ok(()), cell.set(42));
        assert_eq!(Err(24), cell.set(24));
    }

    #[tokio::test]
    async fn guard() {
        let cell = WaitableCell::<i32>::new();
        {
            let _guard = cell.set_guard_with(|| 42);
        }
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test]
    async fn guard_no_op() {
        let cell = WaitableCell::<i32>::new();
        {
            let _guard = cell.set_guard_with(|| 42);
            let _ = cell.set(24);
        }
        assert_eq!(&24, cell.wait().await);
    }
}
