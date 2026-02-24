ARG CROSS_BASE_IMAGE
ARG CROSS_DEB_ARCH
FROM $CROSS_BASE_IMAGE

ARG CROSS_DEB_ARCH
RUN dpkg --add-architecture ${CROSS_DEB_ARCH} && \
    apt-get -y update && \
    apt-get install -y pkg-config libseccomp-dev:${CROSS_DEB_ARCH} libzstd-dev:${CROSS_DEB_ARCH} libssl-dev unzip

# Install protoc from official releases to ensure proto3 support
# The apt protobuf-compiler may be too old in the cross base image
RUN curl -sSL https://github.com/protocolbuffers/protobuf/releases/download/v25.1/protoc-25.1-linux-x86_64.zip -o /tmp/protoc.zip && \
    unzip -o /tmp/protoc.zip -d /usr/local bin/protoc && \
    rm /tmp/protoc.zip
