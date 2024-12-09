# Containerd Shim Benchmarks

This directory contains benchmarks for testing various aspects of the containerd shims:

## Running the Benchmarks

First, build and install the shims:
```bash
make build
sudo make install
```

Then, build and load the wasi-demo-app:
```bash
make test-image
make load
make test-image/oci
make load/oci
```

To run all benchmarks:
```bash
cargo bench -p containerd-shim-benchmarks
```

Note: The benchmarks require sudo access to run containerd commands. Make sure you have the necessary permissions configured.
