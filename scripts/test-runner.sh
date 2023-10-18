#!/bin/sh
echo $@
TARGET_DIR="$(dirname $0)/.."
WASMEDGE_PATH=$(dirname $(find "$TARGET_DIR" -name libwasmedge.so | head -n 1) 2>/dev/null || echo "")
sudo -E env LD_LIBRARY_PATH="${WASMEDGE_PATH}" "$@"