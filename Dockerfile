# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.64
ARG XX_VERSION=1.1.0

FROM --platform=$BUILDPLATFORM tonistiigi/xx:${XX_VERSION} AS xx

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION} AS base
COPY --from=xx / /
RUN apt-get update -y && apt-get install --no-install-recommends -y clang jq

FROM base AS build
SHELL ["/bin/bash", "-c"]
ARG BUILD_TAGS TARGETPLATFORM
ENV WASMEDGE_INCLUDE_DIR=/root/.wasmedge/include
ENV WASMEDGE_LIB_DIR=/root/.wasmedge/lib
ENV LD_LIBRARY_PATH=/root/.wasmedge/lib
RUN xx-apt-get install -y gcc g++ libc++6-dev zlib1g
RUN xx-apt-get install -y pkg-config libsystemd-dev libdbus-glib-1-dev build-essential libelf-dev libseccomp-dev libclang-dev
RUN rustup target add $(xx-info march)-unknown-$(xx-info os)-$(xx-info libc)
RUN <<EOT
    set -ex
    os=$(xx-info os)
    curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash -s -- --version=0.11.2 --platform=${os^} --machine=$(xx-info march)
EOT

WORKDIR /build/src
COPY --link crates ./crates
COPY --link Cargo.toml ./
COPY --link Cargo.lock ./
ARG CRATE=""
ARG TARGETOS TARGETARCH TARGETVARIANT
RUN --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/cache \
    --mount=type=cache,target=/usr/local/cargo/registry/index \
    --mount=type=cache,target=/build/src/target,id=runwasi-cargo-build-cache-${TARGETOS}-${TARGETARCH}${TARGETVARIANT} <<EOT
    set -e
    export "CARGO_NET_GIT_FETCH_WITH_CLI=true"
    export "CARGO_TARGET_$(xx-info march | tr '[:lower:]' '[:upper:]' | tr - _)_UNKNOWN_$(xx-info os | tr '[:lower:]' '[:upper:]' | tr - _)_$(xx-info libc | tr '[:lower:]' '[:upper:]' | tr - _)_LINKER=$(xx-info)-gcc"
    export "CC_$(xx-info march | tr '[:lower:]' '[:upper:]' | tr - _)_UNKNOWN_$(xx-info os | tr '[:lower:]' '[:upper:]' | tr - _)_$(xx-info libc | tr '[:lower:]' '[:upper:]' | tr - _)=$(xx-info)-gcc"
    if [ -n "${CRATE}" ]; then
        package="--package=${CRATE}"
    fi
    cargo build --release --target=$(xx-info march)-unknown-$(xx-info os)-$(xx-info libc) ${package}
EOT
COPY scripts ./scripts
RUN --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/cache \
    --mount=type=cache,target=/usr/local/cargo/registry/index \
    --mount=type=cache,target=/build/src/target,id=runwasi-cargo-build-cache-${TARGETOS}-${TARGETARCH}${TARGETVARIANT} <<EOT
    set -e
    mkdir /build/bin
    bins="$(scripts/bins.sh ${CRATE} | jq -r 'join(" ")')"
    echo "Copying binaries: ${bins}"
    for bin in ${bins}; do
        cp target/$(xx-info march)-unknown-$(xx-info os)-$(xx-info libc)/release/${bin} /build/bin/${bin}
    done
EOT

FROM build AS build-tar
WORKDIR /build/release
ARG CRATE
ARG TARGETOS TARGETARCH TARGETVARIANT
RUN tar -C /build/bin -czf /build/release/${CRATE}-${TARGETOS}-${TARGETARCH}${TARGETVARIANT}.tar.gz .

FROM scratch AS release-tar
COPY --link --from=build-tar /build/release/* /

FROM scratch AS release
COPY --link --from=build /build/bin/* /
