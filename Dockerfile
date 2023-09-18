# syntax=docker/dockerfile:1

# Make sure to keep RUST_VERSION in sync with the version in rust-toolchain.toml
ARG BASE_IMAGE="bullseye"
ARG RUST_VERSION=1.72.0
ARG XX_VERSION=1.2.1
ARG CRATE="containerd-shim-wasmtime,containerd-shim-wasmedge,containerd-shim-wasmer"

FROM --platform=$BUILDPLATFORM tonistiigi/xx:${XX_VERSION} AS xx
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-${BASE_IMAGE} AS base
COPY --from=xx / /

COPY ./scripts/dockerfile-utils.sh /usr/bin/dockerfile-utils

# Install host dependencies
RUN dockerfile-utils install_host

# See https://github.com/tonistiigi/xx/issues/108
RUN sed -i -E 's/xx-clang --setup-target-triple/XX_VENDOR=\$vendor xx-clang --setup-target-triple/' $(which xx-cargo) && \
    sed -i -E 's/\$\(xx-info\)-/\$\(XX_VENDOR=\$vendor xx-info\)-/g' $(which xx-cargo)

FROM base AS build
WORKDIR /src

RUN --mount=type=bind,target=/src,rw,source=. \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/cache \
    --mount=type=cache,target=/usr/local/cargo/registry/index \
    CARGO_NET_GIT_FETCH_WITH_CLI="true" \
    cargo fetch

ARG TARGETPLATFORM

RUN dockerfile-utils install_target

ARG CRATE

RUN --mount=type=bind,target=/src,rw,source=. \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/cache \
    --mount=type=cache,target=/usr/local/cargo/registry/index \
    --mount=type=cache,target=/build,id=runwasi-cargo-build-cache-${CRATE}-${BASE_IMAGE}-${TARGETPLATFORM} <<EOT
    set -ex
    . dockerfile-utils setup_build
    for crate in $(echo $CRATE | tr ',' ' '); do
        package="$package --package=${crate}"
    done
    xx-cargo build --release ${package} ${CARGO_FLAGS} --target-dir /build
EOT

FROM build AS package
RUN --mount=type=bind,target=/src,rw,source=. \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/cache \
    --mount=type=cache,target=/usr/local/cargo/registry/index \
    --mount=type=cache,target=/build,id=runwasi-cargo-build-cache-${CRATE}-${BASE_IMAGE}-${TARGETPLATFORM} <<EOT
    set -ex
    mkdir -p /release/tar /release/bin
    export BUILD_DIR="/build/$(xx-cargo --print-target-triple)/release/"
    export VARIANT="$(xx-info os)-$(xx-info arch)$(xx-info variant)-$(xx-info libc)"
    for crate in $(echo $CRATE | tr ',' ' '); do
        BINS="$(scripts/bins.sh ${crate} | jq -r 'join(" ")')"
        if [ -n "${BINS}" ]; then
            tar -C "${BUILD_DIR}" -czf "/release/tar/${crate}-${VARIANT}.tar.gz" ${BINS}
            tar -C /release/bin -xzf "/release/tar/${crate}-${VARIANT}.tar.gz"
        fi
    done
EOT

FROM scratch AS release-tar
COPY --link --from=package /release/tar/* /

FROM scratch AS release
COPY --link --from=package /release/bin/* /
