use anyhow::Result;
use containerd_shim::cgroup::collect_metrics;
use containerd_shim::util::convert_to_any;
use protobuf::well_known_types::any::Any;

pub fn get_metrics(pid: u32) -> Result<Any> {
    let metrics = collect_metrics(pid)?;

    let metrics = convert_to_any(Box::new(metrics))?;
    Ok(metrics)
}
