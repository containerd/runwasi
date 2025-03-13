#![cfg_attr(windows, allow(dead_code))] // this is currently used only for linux

use std::future::Future;
use std::sync::LazyLock;
use std::time::Duration;

use private::Sealded;
use tokio::task::JoinHandle;
use tokio::time::timeout;

// A thread local runtime that can be used to run futures to completion.
// It is a current_thread runtime so that it doesn't spawn new threads.
// It is thread local as different threads might want to run futures concurrently.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
});

mod private {
    pub trait Sealded {}
    impl<F: Future> Sealded for F {}
}

pub trait AmbientRuntime: Future + Sealded {
    fn block_on(self) -> Self::Output
    where
        Self: Sized,
    {
        RUNTIME.block_on(self)
    }

    #[allow(dead_code, async_fn_in_trait)] // used in tests and with the testing feature
    async fn with_timeout(self, t: Duration) -> Option<Self::Output>
    where
        Self: Sized,
    {
        timeout(t, self).await.ok()
    }

    fn spawn(self) -> JoinHandle<Self::Output>
    where
        Self: Sized + Send + 'static,
        Self::Output: Send + 'static,
    {
        RUNTIME.spawn(self)
    }
}

impl<F: Future> AmbientRuntime for F {}
