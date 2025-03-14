use anyhow::Result;
use containerd_shim::util::convert_to_any;
use oci_spec::runtime;
use protobuf::well_known_types::any::Any;

pub fn get_metrics(pid: u32) -> Result<Any> {
    // Create empty message for now
    // https://github.com/containerd/rust-extensions/pull/178
    let m = protobuf::well_known_types::any::Any::new();

    let metrics = convert_to_any(Box::new(m))?;
    Ok(metrics)
}
