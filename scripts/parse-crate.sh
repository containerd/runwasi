#!/bin/bash

if [ -z "$1" ]; then
    echo "Usage: $0 <path-to-main.log>"
    exit 1
fi

log_file="$1"

# extract crate and version from log file
dry_run=false
crate=$(grep 'Release ' "$log_file" | sed 's/.*Release \([a-zA-Z0-9_-]*\).*/\1/')
version=$(grep 'Release ' "$log_file" | sed 's/.* v\(.*\)/\1/')
if grep -q '\[dry-run\]' "$log_file"; then
    dry_run=true
fi

is_binary="false"
is_crate="false"

# Define ground truth for binary crates
declare -A binary_map=(
    ["oci-tar-builder"]=true
    ["containerd-shim-wasmtime"]=true
    ["containerd-shim-wasmer"]=true
    ["containerd-shim-wasmedge"]=true
)

# Define ground truth for crate items
declare -A crate_map=(
    ["oci-tar-builder"]=true
    ["containerd-shim-wasm"]=true
    ["containerd-shim-wasm-test-modules"]=true
)

# Check and assign based on the binary_map
if [[ "${binary_map[$crate]}" == "true" ]]; then
    is_binary="true"
fi

# Check and assign based on the crate_map
if [[ "${crate_map[$crate]}" == "true" ]]; then
    is_crate="true"
fi

# Runtime logic
declare -a non_shim_crates=("containerd-shim-wasm" "containerd-shim-wasm-test-modules" "oci-tar-builder")
runtime=""

if printf '%s\n' "${non_shim_crates[@]}" | grep -q "^$crate$"; then
    runtime="common"
else
    runtime="${crate#containerd-shim-}"
fi
echo "dry_run=$dry_run"
echo "crate=$crate"
echo "version=$version"
echo "is_binary=$is_binary"
echo "is_crate=$is_crate"
echo "runtime=$runtime"