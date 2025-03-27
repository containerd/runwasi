use std::fs::File;
use std::io::Read;

use anyhow::{Context, Result};
use context::{RuntimeContext, Source};
use path::PathResolve as _;

pub mod context;
pub(crate) mod path;

#[trait_variant::make(Send)]
pub trait Sandbox: Default + 'static {
    /// Run a WebAssembly container
    async fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32>;

    /// Check that the runtime can run the container.
    /// This checks runs after the container creation and before the container starts.
    /// By default it checks that the wasi_entrypoint is either:
    /// * a OCI image with wasm layers
    /// * a file with the `wasm` filetype header
    /// * a parsable `wat` file.
    async fn can_handle(&self, ctx: &impl RuntimeContext) -> Result<()> {
        // this async block is required to make the rewrite of trait_variant happy
        async move {
            let source = ctx.entrypoint().source;

            let path = match source {
                Source::File(path) => path,
                Source::Oci(_) => return Ok(()),
            };

            path.resolve_in_path_or_cwd()
                .next()
                .context("module not found")?;

            let mut buffer = [0; 4];
            File::open(&path)?.read_exact(&mut buffer)?;

            if buffer.as_slice() != b"\0asm" {
                // Check if this is a `.wat` file
                wat::parse_file(&path)?;
            }

            Ok(())
        }
    }
}
