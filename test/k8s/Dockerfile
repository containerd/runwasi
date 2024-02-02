# syntax=docker/dockerfile:1.4

ARG KIND_VERSION=v0.21.0
ARG KIND_NODE_VERSION=v1.29.1@sha256:a0cc28af37cf39b019e2b448c54d1a3f789de32536cb5a5db61a49623e527144
ARG RUNTIME=wasmtime

FROM scratch AS kind
ARG TARGETARCH KIND_VERSION
ADD --chmod=755 https://kind.sigs.k8s.io/dl/${KIND_VERSION}/kind-linux-${TARGETARCH} /kind

FROM kindest/node:${KIND_NODE_VERSION}
RUN apt-get update -y && \
    apt-get install --no-install-recommends -y libdbus-1-3

ADD dist/bin/* /usr/local/bin/

ARG RUNTIME
RUN cat <<EOF >> /etc/containerd/config.toml
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]
runtime_type = "io.containerd.${RUNTIME}.v1"
EOF