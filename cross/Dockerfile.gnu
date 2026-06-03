ARG BASE_IMAGE=ubuntu:22.04
FROM ${BASE_IMAGE}

ARG CROSS_DEB_ARCH
ARG CROSS_CMAKE_SYSTEM_PROCESSOR
ARG CROSS_SYSROOT
ARG GNU_TARGET_TRIPLE
ARG GNU_TARGET_PACKAGES
ENV DEBIAN_FRONTEND=noninteractive
RUN native_arch="$(dpkg --print-architecture)" && \
    if [ "${CROSS_DEB_ARCH}" != "${native_arch}" ]; then \
      sed 's/http:\/\/\(.*\).ubuntu.com\/ubuntu\//[arch-=amd64,i386] http:\/\/ports.ubuntu.com\/ubuntu-ports\//g' /etc/apt/sources.list > /etc/apt/sources.list.d/ports.list && \
      sed -i 's/http:\/\/\(.*\).ubuntu.com\/ubuntu\//[arch=amd64,i386] http:\/\/\1.archive.ubuntu.com\/ubuntu\//g' /etc/apt/sources.list && \
      dpkg --add-architecture "${CROSS_DEB_ARCH}"; \
    fi && \
    apt-get -y update && \
    apt-get install -y --no-install-recommends \
      build-essential \
      ca-certificates \
      cmake \
      curl \
      gfortran \
      libclang-dev \
      libseccomp-dev:${CROSS_DEB_ARCH} \
      libssl-dev \
      libzstd-dev:${CROSS_DEB_ARCH} \
      pkg-config \
      unzip \
      ${GNU_TARGET_PACKAGES} && \
    PROTOC_ARCH=$(uname -m) && \
    if [ "$PROTOC_ARCH" = "x86_64" ]; then PROTOC_ARCH="x86_64"; elif [ "$PROTOC_ARCH" = "aarch64" ]; then PROTOC_ARCH="aarch_64"; fi && \
    curl -sSLo /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v25.1/protoc-25.1-linux-${PROTOC_ARCH}.zip" && \
    unzip -o /tmp/protoc.zip -d /usr/local bin/protoc 'include/*' && \
    rm /tmp/protoc.zip && \
    rm -rf /var/lib/apt/lists/*

ENV CROSS_SYSROOT=${CROSS_SYSROOT} \
    PKG_CONFIG_PATH=/usr/lib/${GNU_TARGET_TRIPLE}/pkgconfig \
    PKG_CONFIG_ALLOW_CROSS=1 \
    CROSS_CMAKE_SYSTEM_NAME=Linux \
    CROSS_CMAKE_SYSTEM_PROCESSOR=${CROSS_CMAKE_SYSTEM_PROCESSOR} \
    CROSS_CMAKE_CRT=gnu \
    CROSS_CMAKE_OBJECT_FLAGS="-ffunction-sections -fdata-sections -fPIC" \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_linux_gnu="--sysroot=/usr/aarch64-linux-gnu -idirafter/usr/include" \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc \
    AR_x86_64_unknown_linux_gnu=ar \
    CC_x86_64_unknown_linux_gnu=gcc \
    CXX_x86_64_unknown_linux_gnu=g++
