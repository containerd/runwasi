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

#### Build

```terminal
$ make build
```

#### Install

```terminal
$ sudo make install
```