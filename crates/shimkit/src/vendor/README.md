# Vendored Code

This directory contains vendored code from dependencies that we need to customize or extend.

### Usage

To use the vendored logger instead of the original:

```rust
// Instead of
use containerd_shim::logger;

// Use the vendored version
use crate::vendor::containerd_shim::logger;
```

### Updating Vendored Code

When a new version of `containerd-shim` is released, delete the vendored code and use the new version directly.