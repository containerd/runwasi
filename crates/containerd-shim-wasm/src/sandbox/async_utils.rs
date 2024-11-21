#![cfg_attr(windows, allow(dead_code))] // this is currently used only for linux

use std::future::Future;

thread_local! {
    // A thread local runtime that can be used to run futures to completion.
    // It is a current_thread runtime so that it doesn't spawn new threads.
    // It is thread local as different threads might want to run futures concurrently.
    static RUNTIME: tokio::runtime::Runtime = {
        tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
    };
}

pub trait AmbientRuntime: Future {
    fn block_on(self) -> Self::Output
    where
        Self: Sized,
    {
        RUNTIME.with(|runtime| runtime.block_on(self))
    }
}

impl<F: Future> AmbientRuntime for F {}
