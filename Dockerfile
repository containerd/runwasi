# syntax=docker/dockerfile:1

ARG XX_VERSION=1.2.1
ARG RUST_VERSION=1.69.0

FROM --platform=$BUILDPLATFORM tonistiigi/xx:${XX_VERSION} AS xx
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION} AS base
COPY --from=xx / /
RUN apt-get update -y && apt-get install --no-install-recommends -y clang cmake protobuf-compiler pkg-config dpkg-dev

# See https://github.com/tonistiigi/xx/issues/108
RUN sed -i -E 's/xx-clang --setup-target-triple/XX_VENDOR=\$vendor xx-clang --setup-target-triple/' $(which xx-cargo) && \
    sed -i -E 's/\$\(xx-info\)-/\$\(XX_VENDOR=\$vendor xx-info\)-/g' $(which xx-cargo)

# See https://github.com/rust-lang/cargo/issues/9167
RUN mkdir -p /.cargo && \
    echo '[net]' > /.cargo/config && \
    echo 'git-fetch-with-cli = true' >> /.cargo/config

FROM base as build
ADD . /runwasi
WORKDIR /runwasi

SHELL ["/bin/bash", "-c"]
RUN --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/cache \
    --mount=type=cache,target=/usr/local/cargo/registry/index \
    --mount=type=cache,target=/build/app,id=wasmedge-wasmtime-$TARGETPLATFORM \
    cargo fetch

ARG BUILD_TAGS TARGETPLATFORM
RUN xx-apt-get install -y gcc g++ libc++6-dev zlib1g libdbus-1-dev libseccomp-dev
RUN rustup target add wasm32-wasi

RUN mkdir -p /dist

RUN --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/cache \
    --mount=type=cache,target=/usr/local/cargo/registry/index \
    --mount=type=cache,target=/build,id=containerd-wasi-shims-$TARGETPLATFORM \
    make TARGET=release CARGO=xx-cargo build

RUN --mount=type=cache,target=/build,id=containerd-wasi-shims-$TARGETPLATFORM \
    make TARGET=release CARGO=xx-cargo PREFIX=/dist install

FROM build AS build-tar
ARG TARGETPLATFORM
RUN mkdir -p /dist/tar
RUN tar -C /dist/bin -czf "/dist/tar/runwasi-$(xx-info os)-$(xx-info march).tar.gz" .

FROM scratch AS release
COPY --link --from=build /dist/bin/* /

FROM scratch AS release-tar
COPY --link --from=build-tar /dist/tar/* /
