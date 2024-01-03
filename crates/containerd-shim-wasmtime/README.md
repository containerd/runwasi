## containerd-shim-wasmtime

This is a [containerd] shim for running WebAssembly modules and components using [wasmtime].

[containerd]: https://containerd.io/
[wasmtime]: https://wasmtime.dev/

### WASI

This shim assumes that the Wasm moudles it runs are [WASI] modules. WASI is a system interface for WebAssembly. If no entrypoint is specified, the shim will look for a `_start` function in the module, which is an initial point of execution when the module is loaded in the runtime. The `_start` funciton is a WASI convention for the Command modules (see the distinction between the Command and Reactors [here]).

The shim adds experimental support for running [WASI Preview 2](https://github.com/WebAssembly/WASI/blob/main/preview2/README.md) components. If no entrypoint is specified, the shim will assume that the WASI component is a component that uses the [wasi:cli/command](https://github.com/WebAssembly/wasi-cli) world.

[WASI]: https://wasi.dev/
[here]: https://github.com/WebAssembly/WASI/issues/13