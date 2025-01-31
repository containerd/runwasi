# Shim stress test with containerd

This crate provides a way to stress test the shim.

## Getting started

```bash
cargo run -p stress-test-c8d -- --help
```

Install wasmtime shim
```bash
make build-wasmtime & sudo make install-wasmtime
```

then stress test it
```bash
sudo cargo run -p stress-test-c8d
```