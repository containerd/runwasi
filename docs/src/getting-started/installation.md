# Installation

This guide will help you install and set up Runwasi on your system.

## Prerequisites

Before installing Runwasi, ensure you have the following prerequisites installed:

- [Rust](https://www.rust-lang.org/tools/install) (stable)
- [containerd](https://github.com/containerd/containerd/blob/main/docs/getting-started.md)

Additionally, check the [contributing guide](../CONTRIBUTING.md#setting-up-your-local-environment) for detailed instructions on setting up your environment with all required dependencies.

## Installation Methods

### Option 1: Installing Prebuilt Binaries

The easiest way to get started is to download prebuilt binaries from the [GitHub releases page](https://github.com/containerd/runwasi/releases).

1. Navigate to the [releases page](https://github.com/containerd/runwasi/releases)
2. Download the appropriate shim for your preferred WebAssembly runtime:
   - `containerd-shim-wasmtime-v1` - for Wasmtime runtime
   - `containerd-shim-wasmedge-v1` - for WasmEdge runtime
   - `containerd-shim-wasmer-v1` - for Wasmer runtime
   - `containerd-shim-wamr-v1` - for WebAssembly Micro Runtime (WAMR)

3. Make the binary executable and move it to your PATH:

```bash
chmod +x containerd-shim-wasmtime-v1
sudo install containerd-shim-wasmtime-v1 /usr/local/bin/
```

4. Verify the binary signature (recommended):

```bash
# Verify using cosign
cosign verify-blob \
    --signature containerd-shim-wasmtime-v1.sig \
    --certificate containerd-shim-wasmtime-v1.pem \
    --certificate-identity https://github.com/containerd/runwasi/.github/workflows/action-build.yml@refs/heads/main \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    containerd-shim-wasmtime-v1
```

### Option 2: Building from Source

To build and install Runwasi from source:

1. Clone the repository:

```bash
git clone https://github.com/containerd/runwasi.git
cd runwasi
```

2. Build the shim for your preferred runtime:

```bash
make build
```

> Note: `make build` will only build shims for all runtimes. You can specify which runtime to build with `make build-wasmtime`, `make build-wasmer`, `make build-wasmedge`, `make build-wamr` etc.

3. Install the binary:

```bash
sudo make install
```

The `make install` command copies the binary to $PATH

## Testing Your Installation

After installation, you can test your setup by pulling and running a test image:

1. Pull the test image:

```bash
sudo ctr images pull ghcr.io/containerd/runwasi/wasi-demo-app:latest
```

2. Run a test container:

```bash
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm
```

You should see output from the demo application.

## Next Steps

Now that you have runwasi shims installed, you can proceed to the [Demos](./demos.md) to learn how to run WebAssembly workloads with Runwasi.
