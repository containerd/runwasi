# Shim stress test

This crate provides a way to stress test the shim.

## Getting started

```bash
cargo run -p stress-test -- --help
```

Build some shim
```bash
make build-wasmtime
```

then stress test it
```bash
cargo run -p stress-test -- $PWD/target/x86_64-unknown-linux-gnu/debug/containerd-shim-wasmtime-v1
```