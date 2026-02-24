ARG CROSS_BASE_IMAGE
ARG CROSS_DEB_ARCH
FROM $CROSS_BASE_IMAGE

ARG CROSS_DEB_ARCH

# Install newer libclang from LLVM apt repo (cross base images use Ubuntu 16.04
# with libclang 3.8 which is too old for bindgen/clang-sys >= 6.0 requirement)
RUN apt-get -y update && \
    apt-get install -y wget gnupg software-properties-common && \
    wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | apt-key add - && \
    echo "deb http://apt.llvm.org/xenial/ llvm-toolchain-xenial-9 main" >> /etc/apt/sources.list && \
    apt-get -y update && \
    apt-get install -y libclang-9-dev

ENV LIBCLANG_PATH="/usr/lib/llvm-9/lib"

RUN dpkg --add-architecture ${CROSS_DEB_ARCH} && \
    apt-get -y update && \
    apt-get install -y pkg-config libseccomp-dev:${CROSS_DEB_ARCH} libzstd-dev:${CROSS_DEB_ARCH} libssl-dev unzip

# Install protoc from official releases to ensure proto3 support
# The apt protobuf-compiler may be too old in the cross base image
RUN curl -sSL https://github.com/protocolbuffers/protobuf/releases/download/v25.1/protoc-25.1-linux-x86_64.zip -o /tmp/protoc.zip && \
    unzip -o /tmp/protoc.zip -d /usr/local bin/protoc && \
    rm /tmp/protoc.zip
