# syntax=docker/dockerfile:1.4

ARG KIND_NODE_VERSION=v1.23.13

FROM kindest/node:${KIND_NODE_VERSION} AS kind-base

# We need the build to link using against same system libs as the kind image otherwise the shim won't work.
# So use the node image here as a base and install rust on top of it.
FROM kind-base AS shim-build
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs > /tmp/rustup.sh && sh /tmp/rustup.sh -y --profile=minimal
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup install stable
WORKDIR /shim
COPY . .
RUN apt-get update && apt-get install --no-install-recommends -y build-essential git clang pkg-config libsystemd-dev libdbus-glib-1-dev build-essential libelf-dev libseccomp-dev libclang-dev
RUN curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash -s -- --version=0.11.2
RUN \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/shim/target \
    mkdir -p /opt/shim; make build install PREFIX=/opt/shim

FROM scratch AS shim
COPY --from=shim-build /opt/shim/bin/* /


# Reuse kind-base so we don't have to download other images...
FROM kind-base AS kind-fetch
ARG TARGETARCH
ARG KIND_VERSION=v0.17.0
RUN curl -sSLf https://kind.sigs.k8s.io/dl/${KIND_VERSION}/kind-linux-${TARGETARCH} > /root/kind && chmod +x /root/kind

FROM scratch AS kind
COPY --from=kind-fetch /root/kind /kind

FROM kind-base
RUN <<EOF
set -e
echo '[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]' >> /etc/containerd/config.toml
echo 'runtime_type = "io.containerd.wasmtime.v1"' >> /etc/containerd/config.toml
sed -i 's,SystemdCgroup = true,,' /etc/containerd/config.toml
EOF
COPY --link --from=shim /* /usr/local/bin/

