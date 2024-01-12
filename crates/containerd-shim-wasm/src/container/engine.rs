use std::fs::File;
use std::io::Read;

use anyhow::{Context, Result};

use super::Source;
use crate::container::{PathResolve, RuntimeContext};
use crate::sandbox::Stdio;

pub trait Engine: Clone + Send + Sync + 'static {
    /// The name to use for this engine
    fn name() -> &'static str;

    /// Run a WebAssembly container
    fn run_wasi(&self, ctx: &impl RuntimeContext, wasm_bytes: &[u8], stdio: Stdio) -> Result<i32>;

    /// Check that the runtime can run the container.
    /// This checks runs after the container creation and before the container starts.
    /// By it checks that the wasi_entrypoint is either:
    /// * a OCI image with wasm layers
    /// * a file with the `wasm` filetype header
    /// * a parsable `wat` file.
    fn can_handle(&self, ctx: &impl RuntimeContext) -> Result<()> {
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

    /// Return the supported OCI layer types
    /// This is used to filter only layers that are supported by the runtime.
    /// The default implementation returns the OCI layer type 'application/vnd.bytecodealliance.wasm.component.layer.v0+wasm'
    /// for WASM modules which can be contain with wasip1 or wasip2 components.
    /// Runtimes can override this to support other layer types
    /// such as lays that contain runtime specific configuration
    fn supported_layers_types() -> &'static [&'static str] {
        &["application/vnd.bytecodealliance.wasm.component.layer.v0+wasm"]
    }
}
