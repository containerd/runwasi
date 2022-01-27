FROM --platform=${BUILDPLATFORM} tinygo/tinygo:0.21.0 AS build
WORKDIR /opt/wasmtest
COPY wasm.go .
RUN tinygo build --target=wasi -o wasm wasm.go

FROM scratch
ENTRYPOINT /wasm
COPY --from=build /opt/wasmtest/wasm /