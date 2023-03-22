![runwasi logo light mode](./art/logo/runwasi_icon1.svg#gh-light-mode-only)
![runwasi logo dark mode](./art/logo/runwasi_icon3.svg#gh-dark-mode-only)

## runwasi

> Warning: Alpha quality software, do not use in production.

This is a project to facilitate running wasm workloads managed by containerd either directly (ie. through ctr) or as directed by Kubelet via the CRI plugin.
It is intended to be a (rust) library that you can take and integrate with your wasm host.
Included in the repository is a PoC for running a plain wasi host (ie. no extra host functions except to support wasi system calls).

### Community

Come join us on our [slack channel #runwasi](https://cloud-native.slack.com/archives/C04LTPB6Z0V)
on the CNCF slack.

### Usage

runwasi is intended to be consumed as a library to be linked to from your own wasm host implementation.

There are two modes of operation supported:

1. "Normal" mode where there is 1 shim process per container or k8s pod.
2. "Shared" mode where there is a single manager service running all shims in process.

In either case you need to implement the `Instance` trait:

```rust
pub trait Instance {
    /// Create a new instance
    fn new(id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self;
    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, Error>;
    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error>;
    /// delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), Error>;
    /// wait for the instance to exit
    /// The sender is used to send the exit code and time back to the caller
    /// Ideally this would just be a blocking call with a normal result, however
    /// because of how this is called from a thread it causes issues with lifetimes of the trait implementer.
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

- **containerd-shim-[ wasmedge | wasmtime ]-v1**

This is a containerd shim which runs wasm workloads in [WasmEdge](https://github.com/WasmEdge/WasmEdge) or [Wasmtime](https://github.com/bytecodealliance/wasmtime).
You can use it with containerd's `ctr` by specifying `--runtime=io.containerd.[ wasmedge | wasmtime ].v1` when creating the container.
And make sure the shim binary must be in $PATH (that is the $PATH that containerd sees). Usually you just run `make install` after `make build`.
> build shim with wasmedge we need install library first

This shim runs one per pod.

- **containerd-shim-[ wasmedge | wasmtime ]d-v1**

A cli used to connect containerd to the `containerd-[ wasmedge | wasmtime ]d` sandbox daemon.
When containerd requests for a container to be created, it fires up this shim binary which will connect to the `containerd-[ wasmedge | wasmtime ]d` service running on the host.
The service will return a path to a unix socket which this shim binary will write back to containerd which containerd will use to connect to for shim requests.
This binary does not serve requests, it is only responsible for sending requests to the `contianerd-[ wasmedge | wasmtime ]d` daemon to create or destroy sandboxes.

- **containerd-[ wasmedge | wasmtime ]d**

This is a sandbox manager that enables running 1 wasm host for the entire node instead of one per pod (or container).
When a container is created, a request is sent to this service to create a sandbox.
The "sandbox" is a containerd task service that runs in a new thread on its own unix socket, which we return back to containerd to connect to.

The Wasmedge / Wasmtime engine is shared between all sandboxes in the service.

To use this shim, specify `io.containerd.[ wasmedge | wasmtime ]d.v1` as the runtime to use.
You will need to make sure the `containerd-[ wasmedge | wasmtime ]d` daemon has already been started.

#### Test and demo with containerd

**Attention**

Instead of enabling docker-desktop official released feature `use containerd for pulling and storing images`, you can build a local image and interact with the container locally.

- **Install WasmEdge first (If you choose Wasmedge as your wasm runtime)**

    - Install WasmEdge
    - Make sure the library is in the search path.


```terminal
$ curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash
$ sudo -E sh -c 'echo "$HOME/.wasmedge/lib" > /etc/ld.so.conf.d/libwasmedge.conf'
$ sudo ldconfig
```

- **Run unit test**

```terminal
$ cargo test -- --nocapture
```
You should see some output like:
```terminal
running 3 tests
test instance::tests::test_maybe_open_stdio ... ok
test instance::wasitest::test_delete_after_create ... ok
test instance::wasitest::test_wasi ... ok
```

- **Build and install shim components**

```terminal
$ make build
$ sudo make install
```

- **Demo**

Now you can use the test image provided in this repo to have test with, use `make load` to load it into containerd.

- Case 1.

Run it with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime ].v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm /wasi-demo-app.wasm echo 'hello'`. You should see some output repeated like:
```terminal
$ sudo ctr run --rm --runtime=io.containerd.wasmedge.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm /wasi-demo-app.wasm echo 'hello'

hello
exiting
```

- Case 2.

Run it with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime ].v1 docker.io/library/wasmtest:latest testwasm`.
You should see some output repeated like:

```terminal
$ sudo ctr run --rm --runtime=io.containerd.wasmedge.v1 docker.io/library/wasmtest:latest testwasm

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

To kill the process from the case 2. demo, you can run in other session: `sudo ctr task kill -s SIGKILL testwasm`. And the test binary supports full commands, check [test/image/src/main.rs](test/image/src/main.rs) to play around more.
