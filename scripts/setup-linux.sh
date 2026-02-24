#!/bin/bash
sudo apt -y update
sudo apt install -y pkg-config libsystemd-dev libdbus-glib-1-dev build-essential libelf-dev libseccomp-dev libclang-dev libzstd-dev libssl-dev

# Install a newer protoc (>=3.15 required for proto3 optional fields used by containerd-client 0.8+)
PROTOC_VERSION=25.1
ARCH=$(dpkg --print-architecture)
if [ "$ARCH" = "amd64" ]; then
    PROTOC_ARCH="x86_64"
elif [ "$ARCH" = "arm64" ]; then
    PROTOC_ARCH="aarch_64"
else
    PROTOC_ARCH="$ARCH"
fi
curl -sSLo /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${PROTOC_ARCH}.zip"
sudo unzip -o /tmp/protoc.zip -d /usr/local bin/protoc 'include/*'
rm /tmp/protoc.zip

if [ ! -z "$CI" ] && ! mount | grep cgroup; then
    echo "cgroup is not mounted" 1>&2
    exit 1
fi