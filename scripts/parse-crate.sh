#!/bin/bash

crate_name=$1
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
if [[ "${binary_map[$crate_name]}" == "true" ]]; then
    is_binary="true"
fi

# Check and assign based on the crate_map
if [[ "${crate_map[$crate_name]}" == "true" ]]; then
    is_crate="true"
fi

echo "is_binary=$is_binary"
echo "is_crate=$is_crate"