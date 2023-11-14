![runwasi logo light mode](./art/logo/runwasi_icon1.svg#gh-light-mode-only)
![runwasi logo dark mode](./art/logo/runwasi_icon3.svg#gh-dark-mode-only)

## runwasi

> Warning: Alpha quality software, do not use in production.

This is a project to facilitate running wasm workloads managed by containerd either directly (ie. through ctr) or as directed by Kubelet via the CRI plugin.
It is intended to be a (rust) library that you can take and integrate with your wasm host.
Included in the repository is a PoC for running a plain wasi host (ie. no extra host functions except to support wasi system calls).

### Community

- If you haven't joined the CNCF slack yet, you can do so [here](https://slack.cncf.io/).
- Come join us on our [slack channel #runwasi](https://cloud-native.slack.com/archives/C04LTPB6Z0V)
on the CNCF slack.
- Public Community Call on Tuesdays every other week at 9:00 AM PT: [Zoom](https://zoom.us/my/containerd?pwd=bENmREpnSGRNRXdBZWV5UG8wbU1oUT09), [Meeting Notes](https://docs.google.com/document/d/1aOJ-O7fgMyRowHD0kOoA2Z_4d19NyAvvdqOkZO3Su_M/edit?usp=sharing)

### Usage

runwasi is intended to be consumed as a library to be linked to from your own wasm host implementation.

There are two modes of operation supported:

1. "Normal" mode where there is 1 shim process per container or k8s pod.
2. "Shared" mode where there is a single manager service running all shims in process.

In either case you need to implement a trait to teach runwasi how to use your wasm host.

There are two ways to do this:
* implementing the `sandbox::Instance` trait
* or implementing the `container::Engine` trait

The most flexible but complex is the `sandbox::Instance` trait:

```rust
pub trait Instance {
    /// The WASI engine type
    type Engine: Send + Sync + Clone;

    /// Create a new instance
    fn new(id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self;
    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, Error>;
    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error>;
    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), Error>;
    /// Wait for the instance to exit
    /// The waiter is used to send the exit code and time back to the caller
    /// Ideally this would just be a blocking call with a normal result, however
    /// because of how this is called from a thread it causes issues with lifetimes of the trait implementer.
    fn wait(&self, waiter: &Wait) -> Result<(), Error>;
}
```

The `container::Engine` trait provides a simplified API:

```rust
pub trait Engine: Clone + Send + Sync + 'static {
    /// The name to use for this engine
    fn name() -> &'static str;
    /// Run a WebAssembly container
    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32>;
    /// Check that the runtime can run the container.
    /// This checks runs after the container creation and before the container starts.
    /// By it checks that the wasi_entrypoint is either:
    /// * a file with the `wasm` filetype header
    /// * a parsable `wat` file.
    fn can_handle(&self, ctx: &impl RuntimeContext) -> Result<()> { /* default implementation*/ }
}
```

After implementing `container::Engine` you can use `container::Instance<impl container::Engine>`, which implements the `sandbox::Instance` trait.

To use your implementation in "normal" mode, you'll need to create a binary which has a main that looks something like this:

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

or when using the `container::Engine` trait, like this:

```rust
use containerd_shim as shim;
use containerd_shim_wasm::{sandbox::ShimCli, container::{Instance, Engine}}

struct MyEngine {
    // ...
}

impl Engine for MyEngine {
    // ...
}

fn main() {
    shim::run::<ShimCli<Instance<Engine>>>("io.containerd.myshim.v1", opts);
}
```

Note you can implement your own ShimCli if you like and customize your wasm engine and other things.
I encourage you to checkout how that is implemented.

The shim binary just needs to be installed into `$PATH` (as seen by the containerd process) with a binary name like `containerd-shim-myshim-v1`.

For the shared mode:

```rust
use containerd_shim_wasm::sandbox::{Local, ManagerService, Instance};
use containerd_shim_wasm::services::sandbox_ttrpc::{create_manager, Manager};
use std::sync::Arc;
use ttrpc::{self, Server};
/// ...

struct MyInstance {
    /// ...
}

impl Instance for MyInstance {
    // ...
}

fn main() {
    let s: ManagerService<Local<MyInstance>> =
        ManagerService::new(Engine::new(Config::new().interruptable(true)).unwrap());
    let s = Arc::new(Box::new(s) as Box<dyn Manager + Send + Sync>);
    let service = create_manager(s);

    let mut server = Server::new()
        .bind("unix:///run/io.containerd.myshim.v1/manager.sock")
        .unwrap()
        .register_service(service);

    server.start().unwrap();
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    rx.recv().unwrap();
}
```

This will be the host daemon that you startup and manage on your own.
You can use the provided `containerd-shim-myshim-v1` binary as the shim to specify in containerd.

Shared mode requires precise control over real threads and as such should not be used with an async runtime.

### Examples

#### Components

- **containerd-shim-[ wasmedge | wasmtime | wasmer ]-v1**

This is a containerd shim which runs wasm workloads in [WasmEdge](https://github.com/WasmEdge/WasmEdge) or [Wasmtime](https://github.com/bytecodealliance/wasmtime) or [Wasmer](https://github.com/wasmerio/wasmer).
You can use it with containerd's `ctr` by specifying `--runtime=io.containerd.[ wasmedge | wasmtime | wasmer ].v1` when creating the container.
And make sure the shim binary must be in $PATH (that is the $PATH that containerd sees). Usually you just run `make install` after `make build`.
> build shim with wasmedge we need install library first

This shim runs one per pod.

- **containerd-shim-[ wasmedge | wasmtime | wasmer ]d-v1**

A cli used to connect containerd to the `containerd-[ wasmedge | wasmtime | wasmer ]d` sandbox daemon.
When containerd requests for a container to be created, it fires up this shim binary which will connect to the `containerd-[ wasmedge | wasmtime | wasmer ]d` service running on the host.
The service will return a path to a unix socket which this shim binary will write back to containerd which containerd will use to connect to for shim requests.
This binary does not serve requests, it is only responsible for sending requests to the `contianerd-[ wasmedge | wasmtime | wasmer ]d` daemon to create or destroy sandboxes.

- **containerd-[ wasmedge | wasmtime | wasmer ]d**

This is a sandbox manager that enables running 1 wasm host for the entire node instead of one per pod (or container).
When a container is created, a request is sent to this service to create a sandbox.
The "sandbox" is a containerd task service that runs in a new thread on its own unix socket, which we return back to containerd to connect to.

The Wasmedge / Wasmtime / Wasmer engine is shared between all sandboxes in the service.

To use this shim, specify `io.containerd.[ wasmedge | wasmtime | wasmer ]d.v1` as the runtime to use.
You will need to make sure the `containerd-[ wasmedge | wasmtime | wasmer ]d` daemon has already been started.

### Contributing

#### Building

1. Install dependencies

If on ubuntu/debian you can use the following script. Refer to [youki's](https://github.com/containers/youki#dependencies) documentation for other systems.

```
./scripts/setup-linux.sh
```

If on Windows use (use [git BASH](https://gitforwindows.org/) terminal which has shell emulator)

```
./scripts/setup-windows.sh
```

2. Build

```
make build
make check
```

### Running unit tests

```terminal
make test
```

You should see some output like:
```terminal
running 3 tests
test instance::tests::test_maybe_open_stdio ... ok
test instance::wasitest::test_delete_after_create ... ok
test instance::wasitest::test_wasi ... ok
```

### Building the test image

This builds a [wasm application](crates/wasi-demo-app/) and packages it in an OCI format:

```
make test-image
```

### Running integration tests with k3s

```
make test/k3s-[ wasmedge | wasmer ]
```

### Running integration tests with kind

```
make test/k8s
```

## Demo

### Installing the shims for use with Containerd

Make sure you have [installed dependencies](#Building) and install the shims:

```terminal
make build
sudo make install
```

Build the test image and load it into contianerd:

```
make test-image
make load
```

#### Demo 1 using wasmedge

Run it with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime ].v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm /wasi-demo-app.wasm echo 'hello'`. You should see some output repeated like:

```terminal
sudo ctr run --rm --runtime=io.containerd.wasmedge.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm /wasi-demo-app.wasm echo 'hello'

hello
exiting
```

#### Demo 2 using wasmtime

Run it with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime ].v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm`.
You should see some output repeated like:

```terminal
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm

This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...

This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...

(...)
```

#### Demo 3 using wasmer

Run it with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime | wasmer ].v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm`.
You should see some output repeated like:

```terminal
sudo ctr run --rm --runtime=io.containerd.wasmer.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm

This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...

This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...

(...)
```

To kill the process from demo 2, you can run in other session: `sudo ctr task kill -s SIGKILL testwasm`. 

The test binary supports commands for different type of functionality, check [crates/wasi-demo-app/src/main.rs](crates/wasi-demo-app/src/main.rs) to try it out.

## Demo 3 using OCI Images with custom WASM layers

The previous demos run with an OCI Container image containing the wasm module in the file system.  Another option is to provide a cross-platform OCI Image that that will not have the wasm module or components in the file system of the container that wraps the wasmtime/wasmedge process.  This OCI Image with custom WASM layers can be run across any platform and provides for de-duplication in the Containerd content store among other benefits.

To learn more about this approach checkout the [design document](https://docs.google.com/document/d/11shgC3l6gplBjWF1VJCWvN_9do51otscAm0hBDGSSAc/edit).

> **Note**: This requires containerd 1.7.7+ and 1.6.25+ (not yet released).  If you do not have these patches for both `containerd` and `ctr` you will end up with an error message such as `mismatched image rootfs and manifest layers` at the import and run steps.

Build and import the OCI image with WASM layers image:

```
make test-image/oci
make load/oci
```

Run the image with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime | wasmer ].v1 ghcr.io/containerd/runwasi/wasi-demo-oci:latest testwasmoci`

```
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-oci:latest testwasmoci wasi-demo-oci.wasm echo 'hello'
hello
exiting 
```
