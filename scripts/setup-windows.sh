#!/bin/bash
choco install -y wasmedge --version 0.13.1
# require clang for wasmedge for bindgen, which is used in the build script to generate the rust bindings to the c codebase 
choco install -y llvm --version 16.0.6
