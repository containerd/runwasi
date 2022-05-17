## runwasi

**Warning** Alpha quality software, do not use in production.

This is a project to facilitate running wasm workloads managed by containerd either directly (ie. through ctr) or as directed by Kubelet via the CRI plugin.
It is intended to be a (rust) library that you can take and integrate with your wasm host.
Included in the repository is a PoC for running a plain wasi host (ie. no extra host functions except to support wasi system calls).

### Usage

runwasi is intended to be consumed as a library to be linked to from your own wasm host implementation.

There are two modes of operation supported:

1. "Normal" mode where there is 1 shim process per container or k8s pod.
2. "Shared" mode where there is a single manager service running all shims in process.

In either case you need to implement the `Instance` trait:

```rust
pub trait Instance {
    // Create a new instance
    fn new(id: String, cfg: &InstanceConfig) -> Self;
    // Start the instance and return the pid
    fn start(&self) -> Result<u32, Error>;
    // Send the specified signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error>;
    // Delete the instance
    fn delete(&self) -> Result<(), Error>;
    // wait for the instance to exit and send the exit code and exit timestamp to the provided sender.
    fn wait(&self, send: Sender<(u32, DateTime<Utc>)>) -> Result<(), Error>;
}
```

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

Note you can implement your own ShimCli if you like and customize your wasmtime engine and other things.
I encourage you to checkout how that is implemented.

The shim binary just needs to be installed into `$PATH` (as seen by the containerd process) with a binary name like `containerd-shim-myshim-v1`.

For the shared mode:

```rust
use containerd_shim_wasm::sandbox::{Local, ManagerService, Instance};
use containerd_shim_wasm::services::sandbox_ttrpc::{create_manager, Manager};
use std::sync::Arc;
use ttrpc::{self, Server};
use wasmtime::{Config, Engine};

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
        .bind("unix:///run/io.containerd.wasmtime.v1/manager.sock")
        .unwrap()
        .register_service(service);

    server.start().unwrap();
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    rx.recv().unwrap();
}
```

This will be the host daemon that you startup and manage on your own.
You can use the provided `containerd-shim-wasmtimed-v1` binary as the shim to specify in containerd.

Shared mode requires precise control over real threads and as such should not be used with an async runtime.

### Examples
#### containerd-shim-wasmtime-v1

This is a containerd shim which runs wasm workloads in wasmtime.
You can use it with containerd's `ctr` by specifying `--runtime=io.containerd.wasmtime.v1` when creating the container.
The shim binary must be in $PATH (that is the $PATH that containerd sees).

You can use the test image provided in this repo to have test with, use `make load` to load it into containerd.
Run it with `ctr run --rm --runtime=io.containerd.wasmtime.v1 docker.io/library/wasmtest:latest testwasm`.
You should see some output like:
```
Hello from wasm!
```

The test binary supports some other commands, see test/image/wasm.go to play around more.

This shim runs one per pod.

#### containerd-shim-wasmtimed-v1

A cli used to connect containerd to the `containerd-wasmtimed` sandbox daemon.
When containerd requests for a container to be created, it fires up this shim binary which will connect to the `containerd-wasmtimed` service running on the host.
The service will return a path to a unix socket which this shim binary will write back to containerd which containerd will use to connect to for shim requests.
This binary does not serve requests, it is only responsible for sending requests to the `contianerd-wasmtimed` daemon to create or destroy sandboxes.
#### containerd-wasmtimed

This is a sandbox manager that enables running 1 wasm host for the entire node instead of one per pod (or container).
When a container is created, a request is sent to this service to create a sandbox.
The "sandbox" is a containerd task service that runs in a new thread on its own unix socket, which we return back to containerd to connect to.

The wasmtime engine is shared between all sandboxes in the service.

To use this shim, specify `io.containerd.wasmtimed.v1` as the runtime to use.
You will need to make sure the containerd-wasmtimed daemon has already been started.

#### Build

```terminal
$ make build
```

#### Install

```terminal
$ sudo make install
```