//! Generic helpers for working with OCI specs that can be consumed by any runtime.

use oci_spec::image::Descriptor;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WasmLayer {
    pub config: Descriptor,
    #[serde(with = "serde_bytes")]
    pub layer: Vec<u8>,
}
