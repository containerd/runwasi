use anyhow::Result;
use containerd_shim::{self as shim};
use protobuf::well_known_types::any::Any;
use shim::cgroup::collect_metrics;
use shim::util::convert_to_any;

use crate::sandbox::Error;

pub fn get_metrics(pid: u32) -> Result<Any> {
    let metrics = collect_metrics(pid)?;

    let metrics = convert_to_any(Box::new(metrics)).map_err(|e| Error::Others(e.to_string()))?;
    Ok(metrics)
}
