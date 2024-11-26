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

To generate the package and import to a registry using a tool such as [regctl](https://github.com/regclient/regclient/blob/main/docs/install.md).  To [run a local image registry](https://www.docker.com/blog/how-to-use-your-own-registry-2/) use `docker run -d -p 5000:5000 --name registry registry:2.7`

```
cargo run --bin oci-tar-builder -- --name wasi-demo-oci --repo ghcr.io/containerd/runwasi --tag latest --module ./target/wasm32-wasip1/debug/wasi-demo-app.wasm -o ./dist/img-oci.tar
regctl image import localhost:5000/wasi-demo-oci:latest ./dist/img-oci.tar        
```

View the manifest created, notice that the media types for the layers are `application/vnd.bytecodealliance.wasm.component.layer.v0+wasm` which are subject to change.

```
Name:                                localhost:5000/wasi-demo-oci:latest
MediaType:                           application/vnd.oci.image.manifest.v1+json
Digest:                              sha256:6c48b431d29a1ea1ece13fa50e9f33e4d164e07f6a501dbed668aed947002c5c
Annotations:
  io.containerd.image.name:          ghcr.io/containerd/runwasi/wasi-demo-oci:latest
  org.opencontainers.image.ref.name: latest
Total Size:                          2.590MB

Config:
  Digest:                            sha256:beb7483682ae4ec45d02cd7cee8ee733f8dc610cb7e91070dc8f10567365bdd7
  MediaType:                         application/vnd.oci.image.config.v1+json
  Size:                              138B

Layers:

  Digest:                            sha256:656e978ae0c37156a6abe06052a588e5c700346650765859981ebd2089cffd42
  MediaType:                         application/vnd.bytecodealliance.wasm.component.layer.v0+wasm
  Size:                              2.590MB
```

### Spec

See the [OCI Image Spec](https://github.com/opencontainers/image-spec/blob/bc9c4bd/image-layout.md) for more information on the OCI tar format.

In order to be compatible with Docker, since Docker does not currently support the OCI format, this also includes a `manifest.json` file at the root of the tar that describes the image in a way that Docker can import it.

### Wasm Artifact usage

The CNCF wg-wasm has published and [OCI artifact format](https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/) for packaging wasm modules and components.  The artifact can be produced locally by running the `--as-artifact` flag:

```
cargo run --bin oci-tar-builder -- --name wasi-demo-oci --repo ghcr.io/containerd/runwasi --tag latest --as-artifact --module ./target/wasm32-wasip1/debug/wasi-demo-app.wasm -o target/wasm32-wasip1/debug/img-oci-artifact.tar
regctl image import localhost:5000/wasi-artifact:latest target/wasm32-wasip1/debug/img-oci-artifact.tar
```

The manifest will follow the guidance:

```
regctl manifest get localhost:5000/wasi-artifact:latest

Name:                                localhost:5000/wasi-artifact:latest
MediaType:                           application/vnd.oci.image.manifest.v1+json
Digest:                              sha256:7c31e635b3bef8b6c727a316e9a2dae777dbd184318d66a97da040fb11e37d70
Annotations:
  io.containerd.image.name:          ghcr.io/containerd/runwasi/wasi-demo-oci:latest
  org.opencontainers.image.ref.name: latest
Total Size:                          2.006MB

Config:
  Digest:                            sha256:24f30be41b447bbaf3644dad1e1c23dd28b597f36a5f455399657d65945816ea
  MediaType:                         application/vnd.wasm.config.v0+json
  Size:                              235B

Layers:

  Digest:                            sha256:0db51ed1c94837f422b2259c473758f298eef69605eaae6195bc043e25971e94
  MediaType:                         application/wasm
  Size:                              2.006MB
```

As well as the `config.mediaType` will have the following format:

```
 regctl blob get localhost:5000/wasi-artifact:latest sha256:24f30be41b447bbaf3644dad1e1c23dd28b597f36a5f455399657d65945816ea
{
  "created": "2024-06-25T15:58:49.377917735Z",
  "author": null,
  "architecture": "wasm",
  "os": "wasip1",
  "layerDigests": [
    "sha256:0db51ed1c94837f422b2259c473758f298eef69605eaae6195bc043e25971e94"
  ],
  "component": null
}
```