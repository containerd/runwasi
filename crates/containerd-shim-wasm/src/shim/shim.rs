use std::hash::Hash;

use anyhow::Result;
#[doc(inline)]
pub use containerd_shimkit::sandbox::cli::Version;

use crate::sandbox::Sandbox;
use crate::sandbox::context::WasmLayer;

/// The `Shim` trait provides a simplified API for running WebAssembly containers.
///
/// It handles the lifecycle of the container and OCI spec details for you.
#[trait_variant::make(Send)]
pub trait Shim: Sync + 'static {
    /// The name to use for this shim
    fn name() -> &'static str;

    /// Returns the shim version.
    /// Usually implemented using the [`version!()`](crate::shim::version) macro.
    fn version() -> Version {
        Version::default()
    }

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
pub trait Compiler: Sync {
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

    async fn compile(&self, _layers: &[WasmLayer]) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        unreachable!()
    }
}
