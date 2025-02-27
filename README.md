<picture>
  <source media="(prefers-color-scheme: dark)" srcset="./art/logo/runwasi_icon3.svg">
  <img alt="runwasi logo" src="./art/logo/runwasi_icon1.svg">
</picture>

# runwasi

This is a project to facilitate running wasm workloads managed by containerd either directly (ie. through ctr) or as directed by Kubelet via the CRI plugin.
It is intended to be a (rust) library that you can take and integrate with your wasm host.
Included in the repository is a PoC for running a plain wasi host (ie. no extra host functions except to support wasi system calls).

## Community

- If you haven't joined the CNCF slack yet, you can do so [here](https://slack.cncf.io/).
- Come join us on our [slack channel #runwasi](https://cloud-native.slack.com/archives/C04LTPB6Z0V) on the CNCF slack.
- Public Community Call on Tuesdays every other week at 9:00 AM PT: [Zoom](https://zoom.us/my/containerd?pwd=bENmREpnSGRNRXdBZWV5UG8wbU1oUT09), [Meeting Notes](https://docs.google.com/document/d/1aOJ-O7fgMyRowHD0kOoA2Z_4d19NyAvvdqOkZO3Su_M/edit?usp=sharing)

See our [Community Page](https://containerd.github.io/runwasi/resources/community.html) for more ways to get involved.

## Documentation

For comprehensive documentation, visit our [Documentation Site](https://containerd.github.io/runwasi/).

For `containerd-shim-wasm` crate documentation, visit [containerd-shim-wasm](https://docs.rs/containerd-shim-wasm).

## Quick Start

### Installation

```terminal
make build
sudo make install
```

For detailed installation instructions, see the [Installation Guide](https://containerd.github.io/runwasi/getting-started/installation.html).

### Running an Example

```terminal
# Pull the image
sudo ctr images pull ghcr.io/containerd/runwasi/wasi-demo-app:latest

# Run the example
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm
```

For more examples and detailed usage, see the [Demos](https://containerd.github.io/runwasi/getting-started/demos.html).

## Projects Using Runwasi

Check out these projects that build on top of runwasi:
- [spinkube/containerd-shim-spin](https://github.com/spinkube/containerd-shim-spin)
- [deislabs/containerd-wasm-shims](https://github.com/deislabs/containerd-wasm-shims)

## Contributing

To begin contributing, please read our [Contributing Guide](https://containerd.github.io/runwasi/developer/contributing.html).
