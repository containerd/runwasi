# Containerd Shim Benchmarks

This directory contains benchmarks for testing various aspects of the containerd shims:

## Running the Benchmarks

First, build and install the shims:
```bash
make build
sudo make install
```

Then, pull the wasi-demo-app:
```bash
make pull
```

To run all benchmarks:
```bash
cargo bench -p containerd-shim-benchmarks
```

Note: The benchmarks require sudo access to run containerd commands. Make sure you have the necessary permissions configured.
