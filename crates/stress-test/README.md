# Shim stress test

This crate provides a way to stress test the shim.

## Getting started

```bash
cargo run -p stress-test -- --help
```

```
Usage: stress-test [OPTIONS] <SHIM>

Arguments:
  <SHIM>  Path to the shim binary

Options:
  -v, --verbose                      Show the shim logs in stderr
  -O, --container-output             Show the container output in stdout
  -p, --parallel <PARALLEL>          Number of tasks to create and start concurrently [0 = no limit] [default: 1]
  -S, --serial-steps <SERIAL_STEPS>  Up to what steps to run in series [default: start] [possible values: create, start, wait, delete]
  -n, --count <COUNT>                Number of tasks to run [default: 10]
  -t, --timeout <TIMEOUT>            Runtime timeout [0 = no timeout] [default: 2s]
  -h, --help                         Print help
  -V, --version                      Print version
```

Build some shim
```bash
make build-wasmtime
```

then stress test it
```bash
cargo run -p stress-test -- $PWD/target/x86_64-unknown-linux-gnu/debug/containerd-shim-wasmtime-v1
```