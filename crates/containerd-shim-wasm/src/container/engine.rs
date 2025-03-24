use std::fs::File;
use std::hash::Hash;
use std::io::Read;

use anyhow::{Context, Result};

use super::Source;
use crate::container::{PathResolve, RuntimeContext};
use crate::sandbox::oci::WasmLayer;

/// The `Shim` trait provides a simplified API for running WebAssembly containers.
///
/// It handles the lifecycle of the container and OCI spec details for you.
#[trait_variant::make(Send)]
pub trait Shim: Clone + Send + Sync + 'static {
    /// The name to use for this shim
    fn name() -> &'static str;

    type Sandbox: Sandbox;

    /// When `compiler` returns `Some`, the returned `Compiler` will be used to precompile
    /// the layers before they are run.
    /// Returns the compiler to be used by this engine
    /// to precompile layers.
    async fn compiler() -> Option<impl Compiler> {
        async move { NO_COMPILER }
    }

    /// Return the supported OCI layer types
    /// This is used to filter only layers that are supported by the runtime.
    /// The default implementation returns the OCI layer type 'application/vnd.bytecodealliance.wasm.component.layer.v0+wasm'
    /// for WASM modules which can be contain with wasip1 or wasip2 components.
    /// Runtimes can override this to support other layer types
    /// such as lays that contain runtime specific configuration
    fn supported_layers_types() -> &'static [&'static str] {
        &[
            "application/vnd.bytecodealliance.wasm.component.layer.v0+wasm",
            "application/wasm",
        ]
    }
}

#[trait_variant::make(Send)]
pub trait Compiler: Send + Sync + 'static {
    /// `cache_key` returns a hasable type that will be used as a cache key for the precompiled module.
    ///
    /// the return value should at least include the version of the shim running but could include other information such as
    /// a hash of the version and cpu type and other important information in the validation of being able to use precompiled module.
    /// If the hash doesn't match then the module will be recompiled and cached with the new cache_key.
    ///
    /// This hash will be used in the following way:
    /// "runwasi.io/precompiled/<Shim::name()>/<cache_key>"
    fn cache_key(&self) -> impl Hash;

    /// `compile` passes supported OCI layers to engine for compilation.
    /// This is used to precompile the layers before they are run.
    /// It is called only the first time a module is run and the resulting bytes will be cached in the containerd content store.
    /// The cached, precompiled layers will be reloaded on subsequent runs.
    /// The runtime is expected to return the same number of layers passed in, if the layer cannot be precompiled it should return `None` for that layer.
    /// In some edge cases it is possible that the layers may already be precompiled and None should be returned in this case.
    async fn compile(&self, _layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>>;
}

#[trait_variant::make(Send)]
pub trait Sandbox: Default + Send + Sync + 'static {
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

/// Like the unstable never type, this type can never be constructed.
/// Ideally we should use the never type (`!`), but it's unstable.
/// This type can be used to indicate that an engine doesn't support
/// precompilation
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum NoCompiler {}

#[doc(hidden)]
pub const NO_COMPILER: Option<NoCompiler> = None;

impl Compiler for NoCompiler {
    fn cache_key(&self) -> impl Hash {
        unreachable!()
    }

    async fn compile(
        &self,
        _layers: &[crate::sandbox::WasmLayer],
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        unreachable!()
    }
}
