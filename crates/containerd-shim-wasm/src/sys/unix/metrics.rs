use anyhow::Result;
use containerd_shim::{self as shim};
use protobuf::well_known_types::any::Any;
use shim::cgroup::collect_metrics;
use shim::util::convert_to_any;

pub fn get_metrics(pid: u32) -> Result<Any> {
    let metrics = collect_metrics(pid)?;

    let metrics = convert_to_any(Box::new(metrics))?;
    Ok(metrics)
}
