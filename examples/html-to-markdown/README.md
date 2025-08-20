# HTML to Markdown Converter

This is a Rust WebAssembly component that converts HTML files to Markdown format. It demonstrates how to create WASI components for file processing that can run in the runwasi environment.

## Features

- Converts HTML elements to Markdown syntax
- Handles text formatting (bold, italic)
- Preserves links and code blocks
- Processes lists and headings
- Converts blockquotes

## Building

To build the component, you need `cargo-component` installed:

```bash
cargo install cargo-component
```

Then build the component:

```bash
cd html-to-markdown
cargo component build
```

This will create a WebAssembly component at `../target/wasm32-wasip1/debug/html-to-markdown.wasm`.

## Usage

### With wasmtime

```bash
wasmtime run --dir=. ../target/wasm32-wasip1/debug/html-to-markdown.wasm input.html output.md
```

### With runwasi

The component can be packaged as an OCI image and run with containerd using the wasmtime shim:

```bash
# Build OCI image (from repository root)
cargo run --bin oci-tar-builder -- \
    --name html-to-markdown \
    --repo ghcr.io/containerd/runwasi \
    --tag latest \
    --module target/wasm32-wasip1/debug/html-to-markdown.wasm \
    -o html-to-markdown.tar

# Import and run with containerd
sudo ctr image import html-to-markdown.tar
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 \
    --mount type=bind,src=$PWD/examples,dst=/data \
    ghcr.io/containerd/runwasi/html-to-markdown:latest \
    converter /html-to-markdown.wasm /data/test.html /data/output.md
```

## Example

See `test.html` for a sample HTML file and `test.md` for the converted output.

## Dependencies

- `html2md`: HTML to Markdown conversion library
- `scraper`: HTML parsing library (not currently used but available for advanced parsing)
- `wit-bindgen-rt`: WebAssembly Interface Types runtime for WASI components