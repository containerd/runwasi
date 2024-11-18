#![cfg_attr(windows, allow(dead_code))] // this is currently used only for linux

use std::future::Future;
use std::sync::LazyLock;

static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
});

pub trait AmbientRuntime: Future {
    fn block_on(self) -> Self::Output
    where
        Self: Sized,
    {
        RUNTIME.block_on(self)
    }
}

impl<F: Future> AmbientRuntime for F {}
