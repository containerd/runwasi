## containerd-shim-wasmtime

This is a [containerd] shim for running WebAssembly modules and components using [wasmtime].

[containerd]: https://containerd.io/
[wasmtime]: https://wasmtime.dev/

### WASI

This shim assumes that the Wasm modules it runs are [WASI] modules. WASI is a system interface for WebAssembly. If no
entrypoint is specified, the shim will look for a `_start` function in the module, which is an initial point of
execution when the module is loaded in the runtime. The `_start` function is a WASI convention for the Command modules
(see the [distinction between the Command and Reactors](https://github.com/WebAssembly/WASI/issues/13)).

The shim adds experimental support for running [WASI 0.2](https://wasi.dev/interfaces#wasi-02) Wasm components.
If no entrypoint is specified, the shim will assume that the WASI component is a component that uses the [wasi:cli/command](https://github.com/WebAssembly/wasi-cli) world.


### WASI/HTTP

The `wasmtime-shim` supports [`wasi/http`][1] and can be used to serve requests from a `wasi/http` proxy component. The
shim code will try to detect components targeting `http/proxy`, and start up a hyper server to listen for incoming
connections, and forward the incoming requests to the WASM component for handling.

This behavior is very similar to what the [`wasmtime serve`][2] command currently offers. The server task is terminated
upon receiving a terminate or interrupt signal in the container.

This can be very useful on the Wasm-first platforms to allow instance-per-request isolation:

> Eeach Wasm instance serves only one HTTP request, and then goes away. This is fantastic for security and bug
> mitigation: the blast radius of an exploit or guest-runtime bug is only a single request, and can never see the data
> from other users of the platform or even other requests by the same user. [3]

The server can be customized by setting environment variables passed to the `RuntimeContext`. These variables include:

- `WASMTIME_HTTP_PROXY_SOCKET_ADDR`: Defines the socket address to bind to
  (default: 0.0.0.0:8080).
- `WASMTIME_HTTP_PROXY_BACKLOG`: Defines the maximum number of pending
  connections in the queue (default: 100).

#### Getting Started
First, we need to create a Wasm component that uses `http/proxy`. You can follow the instructions in this [article][4]
to develop a Wasm application using `cargo-component`.

Alternatively, you can use the pre-built [Hello World][5] example from this repository, which responds with
"Hello World" text for GET requests.

- Create an OCI Image

Use `oci-tar-builder` to create an OCI image with our `http-handler`. Assuming our Wasm component is named `wasi-http.wasm`:

```shell
cargo run --bin oci-tar-builder -- \
    --name wasi-demo-http \
    --repo ghcr.io/containerd/runwasi \
    --tag latest --module wasi-http.wasm \
    -o ./dist/wasi-http-img-oci.tar
```

- Pull the image:

```shell
make pull-http
```

- Run the image:

```shell
sudo ctr run --rm --net-host --runtime=io.containerd.wasmtime.v1 \
    ghcr.io/containerd/runwasi/wasi-demo-http:latest wasi-http /wasi-http.wasm
```

- Finally, assuming our handler will respond to `GET` requests at `/`, we can
use `curl` to send a request:

```shell
curl 127.0.0.1:8080
Hello, this is your first wasi:http/proxy world!
```

[WASI]: https://wasi.dev/
[1]: https://github.com/WebAssembly/wasi-http
[2]: https://docs.wasmtime.dev/cli-options.html#serve
[3]: https://cfallin.org/blog/2024/08/27/aot-js/
[4]: https://opensource.microsoft.com/blog/2024/09/25/distributing-webassembly-components-using-oci-registries/
[5]: ../containerd-shim-wasm-test-modules/src/modules//component-hello-world.wasm
