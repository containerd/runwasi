![runwasi logo](https://raw.githubusercontent.com/containerd/runwasi/e251de3307bbdc8bf3229020ea2ae2711f31aafa/art/logo/runwasi_logo_icon.svg)

# containerd-shim-wasm

A library to help build containerd shims for wasm workloads.

## Usage

Implement the `Instance` trait, then call `run`, for example,
```rust,no_run
use std::time::Duration;
use chrono::{DateTime, Utc};

use containerd_shim as shim;
use containerd_shim_wasm::sandbox::{Error, Instance, InstanceConfig, ShimCli};

struct MyInstance {
    // ...
}

impl Instance for MyInstance {
    type Engine = ();

   fn new(_id: String, _cfg: Option<&InstanceConfig<Self::Engine>>) -> Result<Self, Error> {
       todo!();
    }
    fn start(&self) -> Result<u32, Error> {
       todo!();
    }
    fn kill(&self, signal: u32) -> Result<(), Error> {
       todo!();
    }
    fn delete(&self) -> Result<(), Error> {
       todo!();
    }
    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
       todo!();
    }
}

shim::run::<ShimCli<MyInstance>>("io.containerd.myshim.v1", None);
```

containerd expects the shim binary to be installed into `$PATH` (as seen by the containerd process) with a binary name like `containerd-shim-myshim-v1` which maps to the `io.containerd.myshim.v1` runtime which would need to be configured in containerd. It (containerd) also supports specifying a path to the shim binary but needs to be configured to do so.

This crate is not tied to any specific wasm engine.

