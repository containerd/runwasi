use containerd_shim_wasm::sandbox::error;
use containerd_shim_wasm::sandbox::oci;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WasmtimeError {
    #[error("{0}")]
    OciError(#[from] oci::Error),
    #[error("{0}")]
    Error(#[from] error::Error),
    #[error("{0}")]
    Wasi(#[from] wasmtime_wasi::Error),
    #[error("{0}")]
    WasiCommonStringArray(#[from] wasi_common::StringArrayError),
}
