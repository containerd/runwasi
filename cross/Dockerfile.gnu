ARG BASE_IMAGE
ARG CROSS_DEB_ARCH
FROM ${BASE_IMAGE}

ARG CROSS_DEB_ARCH
RUN dpkg --add-architecture ${CROSS_DEB_ARCH} && \
    apt-get -y update && \
    apt-get install -y pkg-config libseccomp-dev:${CROSS_DEB_ARCH} libzstd-dev:${CROSS_DEB_ARCH} libssl-dev libclang-dev unzip curl && \
    PROTOC_ARCH=$(uname -m) && \
    if [ "$PROTOC_ARCH" = "x86_64" ]; then PROTOC_ARCH="x86_64"; elif [ "$PROTOC_ARCH" = "aarch64" ]; then PROTOC_ARCH="aarch_64"; fi && \
    curl -sSLo /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v25.1/protoc-25.1-linux-${PROTOC_ARCH}.zip" && \
    unzip -o /tmp/protoc.zip -d /usr/local bin/protoc 'include/*' && \
    rm /tmp/protoc.zip
