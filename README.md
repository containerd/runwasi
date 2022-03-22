## runwasi

**Warning** Alpha quality software, do not use in production.

This is a project to facilitate running wasm workloads managed by containerd either directly (ie. through ctr) or as directed by Kubelet via the CRI plugin.
It is intended to be a (rust) library that you can take and integrate with your wasm host.
Included in the repository is a PoC for running a plain wasi host (ie. no extra host functions except to support wasi system calls).
### containerd-shim-wasmtime-v1

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

### containerd-shim-wasmtimed-v1

A cli used to connect containerd to the `containerd-wasmtimed` sandbox daemon.
When containerd requests for a container to be created, it fires up this shim binary which will connect to the `containerd-wasmtimed` service running on the host.
The service will return a path to a unix socket which this shim binary will write back to containerd which containerd will use to connect to for shim requests.
This binary does not serve requests, it is only responsible for sending requests to the `contianerd-wasmtimed` daemon to create or destroy sandboxes.
### containerd-wasmtimed

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