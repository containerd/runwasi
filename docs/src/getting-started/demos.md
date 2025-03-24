# Demos

This page provides various demonstrations of running WebAssembly workloads with Runwasi.

## Prerequisites

Before running these demos, make sure you have:

1. Installed the Runwasi shims as described in the [Installation Guide](./installation.md)
2. Installed containerd and its CLI tool `ctr`

## Demo 1: Using Container Image with Wasm Module

This demo runs a WebAssembly module from a regular OCI container image that contains the Wasm module in its filesystem.

### Setup

Pull the test image:

```bash
make pull-app
# or
sudo ctr images pull ghcr.io/containerd/runwasi/wasi-demo-app:latest
```

### Running the Demo

Run the image with your preferred runtime:

```bash
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm
```

You can also specify a particular command:

```bash
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm /wasi-demo-app.wasm echo 'hello'
```

The output should look like:

```
This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...
```

To kill the process, run in another terminal:

```bash
sudo ctr task kill -s SIGKILL testwasm
```

The test binary supports various commands for different types of functionality. Check [crates/wasi-demo-app/src/main.rs](https://github.com/containerd/runwasi/blob/main/crates/wasi-demo-app/src/main.rs) to explore more options.

## Demo 2: Using OCI Images with Custom WASM Layers

This demo showcases a more advanced approach using OCI Images with custom WASM layers. This approach doesn't include the Wasm module in the container's filesystem but instead stores it as a separate layer in the OCI image. This provides better cross-platform support and de-duplication in the Containerd content store.

> **Note**: This requires containerd 2.0+, 1.7.7+ or 1.6.25+. If you don't have these patches for both `containerd` and `ctr`, you'll encounter an error message like `mismatched image rootfs and manifest layers`. Latest versions of k3s and kind have the necessary containerd versions.

### Setup

Pull the OCI image with WASM layers:

```bash
make pull
# or
sudo ctr images pull ghcr.io/containerd/runwasi/wasi-demo-oci:latest
```

### Running the Demo

Run the image with your preferred runtime:

```bash
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-oci:latest testwasmoci wasi-demo-oci.wasm echo 'hello'
```

Expected output:

```
hello
exiting
```

To learn more about this approach, check the [design document](https://docs.google.com/document/d/11shgC3l6gplBjWF1VJCWvN_9do51otscAm0hBDGSSAc/edit).

## Demo 3: Using Wasm OCI Artifact

The [CNCF tag-runtime wasm working group](https://tag-runtime.cncf.io/wgs/wasm/charter/) has defined an [OCI Artifact format for Wasm](https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/). This new artifact type enables usage across projects beyond just Runwasi.

```bash
make test/k8s-oci-wasmtime
```

> Note: We use a Kubernetes cluster to run this demo since containerd's ctr has a bug that results in: `unknown image config media type application/vnd.wasm.config.v0+json`

## WASI/HTTP Demo for Wasmtime Shim

The `wasmtime-shim` includes support for the WASI/HTTP specification. For details on running HTTP-based WebAssembly modules, see the [wasmtime-shim documentation](https://github.com/containerd/runwasi/blob/main/crates/containerd-shim-wasmtime/README.md#WASI/HTTP).

## Using Different WebAssembly Runtimes

All demos can be run using any of the available Runwasi shims by replacing `wasmtime` with the runtime of your choice:

- **Wasmtime**: `io.containerd.wasmtime.v1`
- **WasmEdge**: `io.containerd.wasmedge.v1`
- **Wasmer**: `io.containerd.wasmer.v1`
- **WAMR**: `io.containerd.wamr.v1`

For example:

```bash
sudo ctr run --rm --runtime=io.containerd.wasmedge.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm
```

## Next Steps

- Explore running WebAssembly workloads on Kubernetes in the [Quickstart with Kubernetes](./quickstart.md) guide
- Learn about the [Architecture](../developer/architecture.md) of Runwasi
- Check out [Contributing](../CONTRIBUTING.md) to get involved with the project 