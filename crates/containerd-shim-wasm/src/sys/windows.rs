use anyhow::Result;
use containerd_shim::{self as shim};
use oci_spec::runtime;
use protobuf::well_known_types::any::Any;
use shim::util::convert_to_any;

use crate::sandbox::Error;

pub fn get_metrics(pid: u32) -> Result<Any> {
    // Create empty message for now
    // https://github.com/containerd/rust-extensions/pull/178
    let m = protobuf::well_known_types::any::Any::new();

    let metrics = convert_to_any(Box::new(m)).map_err(|e| Error::Others(e.to_string()))?;
    Ok(metrics)
}

pub fn setup_namespaces(spec: &runtime::Spec) -> Result<()> {
    // noop for now
    Ok(())
}
