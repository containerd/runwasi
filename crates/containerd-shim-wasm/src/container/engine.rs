use std::fs::File;
use std::io::Read;

use anyhow::{Context, Result};

use crate::container::{PathResolve, RuntimeContext};
use crate::sandbox::Stdio;

pub trait Engine: Clone + Send + Sync + 'static {
    /// The name to use for this engine
    fn name() -> &'static str;

    /// Run a WebAssembly container
    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32>;

    /// Check that the runtime can run the container.
    /// This checks runs after the container creation and before the container starts.
    /// By it checks that the wasi_entrypoint is either:
    /// * a file with the `wasm` filetype header
    /// * a parsable `wat` file.
    fn can_handle(&self, ctx: &impl RuntimeContext) -> Result<()> {
        let path = ctx
            .wasi_entrypoint()
            .path
            .resolve_in_path_or_cwd()
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
