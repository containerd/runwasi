use std::collections::BTreeSet;
use std::fs::File;
use std::future::Future;
use std::io::Read;

use anyhow::{bail, Context, Result};

use super::Source;
use crate::container::{PathResolve, RuntimeContext};
use crate::sandbox::oci::WasmLayer;

pub trait Engine: Clone + Send + Sync + 'static {
    /// The name to use for this engine
    fn name() -> &'static str;

    /// Run a WebAssembly container
    fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32>;

    /// Check that the runtime can run the container.
    /// This checks runs after the container creation and before the container starts.
    /// By default it checks that the wasi_entrypoint is either:
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
        &[
            "application/vnd.bytecodealliance.wasm.component.layer.v0+wasm",
            "application/wasm",
        ]
    }

    /// Precompile passes supported OCI layers to engine for compilation
    /// This is used to precompile the layers before they are run and will be called if `can_precompile` returns `true`.
    /// It is called only the first time a module is run and the resulting bytes will be cached in the containerd content store.  
    /// The cached, precompiled layers will be reloaded on subsequent runs.
    fn precompile(
        &self,
        _layers: &[WasmLayer],
    ) -> impl Future<Output = Result<Vec<PrecompiledLayer>>> + Send {
        async move { bail!("precompile not supported") }
    }

    /// Can_precompile lets the shim know if the runtime supports precompilation.
    /// When it returns Some(unique_string) the `unique_string` will be used as a cache key for the precompiled module.
    ///
    /// `unique_string` should at least include the version of the shim running but could include other information such as a hash
    /// of the version and cpu type and other important information in the validation of being able to use precompiled module.  
    /// If the string doesn't match then the module will be recompiled and cached with the new `unique_string`.
    ///
    /// This string will be used in the following way:
    /// "runwasi.io/precompiled/<Engine.name()>/<unique_string>"
    ///
    /// When it returns None the runtime will not be asked to precompile the module.  This is the default value.
    fn can_precompile(&self) -> Option<String> {
        None
    }
}

/// A `PrecompiledLayer` represents the precompiled bytes of a layer and the digests of parent layers (if any) used to process it.
#[derive(Clone)]
pub struct PrecompiledLayer {
    /// The media type this layer represents.
    pub media_type: String,
    /// The bytes of the precompiled layer.
    pub bytes: Vec<u8>,
    /// Digests of this layers' parents.
    pub parents: BTreeSet<String>,
}
