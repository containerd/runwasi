![runwasi logo](../../art/logo/runwasi_logo_icon.svg)

# containerd-shim-wasm

A library to help build containerd shims for wasm workloads.

## Usage

```rust
use containerd_shim as shim;
use containerd_shim_wasm::sandbox::{ShimCli, Instance, Nop}

fn main() {
    shim::run::<ShimCli<Nop>>("io.containerd.nop.v1", opts);
}
```

The above example uses the built-in `Nop` instance which does nothing.
You can build your own instance by implementing the `Instance` trait.

```rust
use containerd_shim as shim;
use containerd_shim_wasm::sandbox::{ShimCli, Instance}

struct MyInstance {
 // ...
}

impl Instance for MyInstance {
    // ...
}

fn main() {
    shim::run::<ShimCli<MyInstance>>("io.containerd.myshim.v1", opts);
}
```

containerd expects the shim binary to be installed into `$PATH` (as seen by the containerd process) with a binary name like `containerd-shim-myshim-v1` which maps to the `io.containerd.myshim.v1` runtime which would need to be configured in containerd. It (containerd) also supports specifying a path to the shim binary but needs to be configured to do so.

This crate is not tied to any specific wasm engine.