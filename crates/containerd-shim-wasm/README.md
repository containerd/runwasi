![runwasi logo](https://raw.githubusercontent.com/containerd/runwasi/e251de3307bbdc8bf3229020ea2ae2711f31aafa/art/logo/runwasi_logo_icon.svg)

# containerd-shim-wasm

A library to help build containerd shims for Wasm workloads.

## Usage

To implement a shim, simply implement the `Shim` and `Sandbox` trait:

```rust,no_run
use containerd_shim_wasm::{
    shim::{Shim, Config, Cli, Version, version},
    sandbox::Sandbox,
    sandbox::context::RuntimeContext,
};
use anyhow::Result;

struct MyShim;

#[derive(Default)]
struct MySandbox;

impl Shim for MyShim {
    type Sandbox = MySandbox;

    fn name() -> &'static str {
        "my-shim"
    }

    fn version() -> Version {
        version!()
    }
}

impl Sandbox for MySandbox {
    async fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
        // Implement your Wasm runtime logic here
        Ok(0)
    }
}

MyShim::run(None);
```

The `Engine` trait provides optional methods you can override:

- `can_handle()` - Validates that the runtime can run the container (checks Wasm file headers by default)
- `supported_layers_types()` - Returns supported OCI layer types 
- `precompile()` - Allows precompilation of Wasm modules
- `can_precompile()` - Indicates if the runtime supports precompilation

The resulting shim uses [Youki](https://github.com/youki-dev/youki)'s `libcontainer` crate to manage the container lifecycle, such as creating the container, starting it, and deleting it, and youki handles container sandbox for you.

### Running the shim

containerd expects the shim binary to be installed into `$PATH` (as seen by the containerd process) with a binary name like `containerd-shim-myshim-v1` which maps to the `io.containerd.myshim.v1` runtime. It can be [configured in containerd](https://github.com/containerd/containerd/blob/main/core/runtime/v2/README.md#configuring-runtimes).

This crate is not tied to any specific wasm engine.

Check out these projects that build on top of runwasi:
- [spinframework/containerd-shim-spin](https://github.com/spinframework/containerd-shim-spin)
