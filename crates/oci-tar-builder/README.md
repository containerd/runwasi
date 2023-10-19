## OCI Tar Builder

This is a library that can be used to build OCI tar archives. It is used by the `wasi-demo-app` crate to build the OCI tar archive that is used to run the demo app.
The current implementation is to support encapsulating the `wasi-demo-app` wasm module as an OCI tar.

### Contributing

We welcome contributions to this to make it more robust, useful, and generally better.

### Library Usage

The library is currently not published to crates.io, so you will need to add the following to your `Cargo.toml`:

```toml
[dependencies]
oci-tar-builder = { git = "https://github.com/containerd/runwasi.git" }
```

See [wasi-demo-app build script](../wasi-demo-app/build.rs) for an example.

### Executable usage

There is an experimental executable that uses the library and can package a wasm module as an OCI image with wasm layers.  See the [OCI WASM in containerd](https://docs.google.com/document/d/11shgC3l6gplBjWF1VJCWvN_9do51otscAm0hBDGSSAc) for more information.

To generate the package and import to a registry using a tool such as [regctl](https://github.com/regclient/regclient/blob/main/docs/regctl.md#image-commands): 

```
cargo run --bin oci-tar-builder -- --name wasi-demo-app --repo localhost:5000 --module ./target/wasm32-wasi/debug/wasi-demo-app.wasm -o ./bin
regctl image import localhost:5000/wasi-demo-oci:module ./bin/wasi-demo-app.tar        
```

View the manifest created, notice that the media types are `application/vnd.w3c.wasm.module.v1+wasm` which are subject to change.

```
regctl manifest get localhost:5000/wasi-demo-oci:module
Name:                                localhost:5000/wasi-demo-oci:module
MediaType:                           application/vnd.oci.image.manifest.v1+json
Digest:                              sha256:869fb6029e26713160d7626dce140f1275f591a694203509cb1e047e746daac8
Annotations:
  io.containerd.image.name:          localhost:5000/wasi-demo-app
  org.opencontainers.image.ref.name: 5000/wasi-demo-app
Total Size:                          2.565MB

Config:
  Digest:                            sha256:707ef07a1143cfdf20af52979d835d5cfc86acc9634edb79d28b89a1edbdc452
  MediaType:                         application/vnd.oci.image.config.v1+json
  Size:                              118B

Layers:

  Digest:                            sha256:b434ff20f62697465e24a52e3573ee9c212e3a171e18e0821bbb464b14fdbbf9
  MediaType:                         application/vnd.w3c.wasm.module.v1+wasm
  Size:                              2.565MB
```

### Spec

See the [OCI Image Spec](https://github.com/opencontainers/image-spec/blob/bc9c4bd/image-layout.md) for more information on the OCI tar format.

In order to be compatible with Docker, since Docker does not currently support the OCI format, this also includes a `manifest.json` file at the root of the tar that describes the image in a way that Docker can import it.