# runwasi Examples

This directory contains example WebAssembly components demonstrating various use cases for the runwasi project.

## Examples

### html-to-markdown

A Rust WebAssembly component that converts HTML files to Markdown format.

**Features:**
- File I/O using WASI
- HTML parsing and Markdown generation
- Demonstrates building components with `cargo-component`

**Usage:**
```bash
cd html-to-markdown
cargo component build
wasmtime run --dir=. ../target/wasm32-wasip1/debug/html-to-markdown.wasm input.html output.md
```

See [`html-to-markdown/README.md`](html-to-markdown/README.md) for detailed documentation.

## Building Examples

All examples use `cargo-component` to build WebAssembly components targeting WASI preview2:

1. Install cargo-component:
   ```bash
   cargo install cargo-component
   ```

2. Build an example:
   ```bash
   cd <example-name>
   cargo component build
   ```

3. Run with wasmtime:
   ```bash
   wasmtime run --dir=. ../target/wasm32-wasip1/debug/<example-name>.wasm [args...]
   ```

## Integration with runwasi

These examples can be packaged as OCI images and run with containerd using the wasmtime shim. See the main project documentation for details on setting up the containerd wasmtime shim.

## Contributing

To add a new example:

1. Create a new directory under `examples/`
2. Initialize with `cargo component new <name> --command`
3. Implement your example functionality
4. Add documentation in a README.md file
5. Update this README to reference your example