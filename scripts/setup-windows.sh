#!/bin/bash
# when we bump wasmedge-sdk version, we may need to update the version here as well
choco install -y wasmedge --version 0.13.5
# require clang for wasmedge for bindgen, which is used in the build script to generate the rust bindings to the c codebase 
choco install -y llvm --version 16.0.6
choco install -y protoc

if [ ! -z "$CI" ]; then
    echo "WASMEDGE_LIB_DIR=C:\Program Files\WasmEdge\lib" >> ${GITHUB_ENV}
    echo "WASMEDGE_INCLUDE_DIR=C:\Program Files\WasmEdge\include" >> ${GITHUB_ENV}
fi