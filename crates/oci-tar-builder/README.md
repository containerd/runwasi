## OCI Tar Builder

This is a library that can be used to build OCI tar archives. It is used by the `wasi-demo-app` crate to build the OCI tar archive that is used to run the demo app.
The currently implementation is to support encapsulating the `wasi-demo-app` wasm module into an OCI tar. It may be possible to use this for other purposes, but that not currently a goal of the project.

### Contributing

We welcome contributions to this to make it more robust, useful, and generally better.

### Usage

The library is currently not published to crates.io, so you will need to add the following to your `Cargo.toml`:

```toml
[dependencies]
oci-tar-builder = { git = "https://github.com/containerd/runwasi.git" }
```

### Spec

See the [OCI Image Spec](https://github.com/opencontainers/image-spec/blob/bc9c4bd/image-layout.md) for more information on the OCI tar format.

In order to be compatible with Docker, since Docker does not currently support the OCI format, this also includes a `manifest.json` file at the root of the tar that describes the image in a way that Docker can import it.