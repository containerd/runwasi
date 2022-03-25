FROM --platform=${BUILDPLATFORM} rust:1.59 AS build
RUN rustup target add wasm32-wasi
WORKDIR /opt/wasmtest
COPY . .
RUN cargo build --target=wasm32-wasi --release

FROM scratch
ENTRYPOINT ["/wasm"]
COPY --from=build /opt/wasmtest/target/wasm32-wasi/release/containerd-shim-wasmtime-demo.wasm /wasm