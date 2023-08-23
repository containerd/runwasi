use std::fs;
use std::path::Path;

use anyhow::Result;
use cgroups_rs::cgroup::get_cgroups_relative_paths_by_pid;
use cgroups_rs::hierarchies::{self};
use cgroups_rs::{Cgroup, Subsystem};
use containerd_shim::{self as shim};
use protobuf::well_known_types::any::Any;
use shim::protos::cgroups::metrics::{
    CPUStat, CPUUsage, MemoryEntry, MemoryStat, Metrics, PidsStat, Throttle,
};
use shim::util::convert_to_any;

use crate::sandbox::Error;

pub fn get_metrics(pid: u32) -> Result<Any> {
    let mut metrics = Metrics::new();
    let hier = hierarchies::auto();

    let cgroup = if hier.v2() {
        let path = format!("/proc/{}/cgroup", pid);
        let content = fs::read_to_string(path)?;
        let content = content.strip_suffix('\n').unwrap_or_default();

        let parts: Vec<&str> = content.split("::").collect();
        let path_parts: Vec<&str> = parts[1].split('/').collect();
        let namespace = path_parts[1];
        let cgroup_name = path_parts[2];
        Cgroup::load(
            hierarchies::auto(),
            format!("/sys/fs/cgroup/{namespace}/{cgroup_name}"),
        )
    } else {
        let path = get_cgroups_relative_paths_by_pid(pid).unwrap();
        Cgroup::load_with_relative_paths(hierarchies::auto(), Path::new("."), path)
    };

    // from https://github.com/containerd/rust-extensions/blob/main/crates/shim/src/cgroup.rs#L97-L127
    for sub_system in Cgroup::subsystems(&cgroup) {
        match sub_system {
            Subsystem::Mem(mem_ctr) => {
                let mem = mem_ctr.memory_stat();
                let mut mem_entry = MemoryEntry::new();
                mem_entry.set_usage(mem.usage_in_bytes);
                let mut mem_stat = MemoryStat::new();
                mem_stat.set_usage(mem_entry);
                mem_stat.set_total_inactive_file(mem.stat.total_inactive_file);
                metrics.set_memory(mem_stat);
            }
            Subsystem::Cpu(cpu_ctr) => {
                let mut cpu_usage = CPUUsage::new();
                let mut throttle = Throttle::new();
                let stat = cpu_ctr.cpu().stat;
                for line in stat.lines() {
                    let parts = line.split(' ').collect::<Vec<&str>>();
                    if parts.len() != 2 {
                        Err(Error::Others(format!("invalid cpu stat line: {}", line)))?;
                    }

                    // https://github.com/opencontainers/runc/blob/dbe8434359ca35af1c1e10df42b1f4391c1e1010/libcontainer/cgroups/fs2/cpu.go#L70
                    match parts[0] {
                        "usage_usec" => {
                            cpu_usage.set_total(parts[1].parse::<u64>().unwrap());
                        }
                        "user_usec" => {
                            cpu_usage.set_user(parts[1].parse::<u64>().unwrap());
                        }
                        "system_usec" => {
                            cpu_usage.set_kernel(parts[1].parse::<u64>().unwrap());
                        }
                        "nr_periods" => {
                            throttle.set_periods(parts[1].parse::<u64>().unwrap());
                        }
                        "nr_throttled" => {
                            throttle.set_throttled_periods(parts[1].parse::<u64>().unwrap());
                        }
                        "throttled_usec" => {
                            throttle.set_throttled_time(parts[1].parse::<u64>().unwrap());
                        }
                        _ => {}
                    }
                }
                let mut cpu_stats = CPUStat::new();
                cpu_stats.set_throttling(throttle);
                cpu_stats.set_usage(cpu_usage);
                metrics.set_cpu(cpu_stats);
            }
            Subsystem::Pid(pid_ctr) => {
                let mut pid_stats = PidsStat::new();
                pid_stats.set_current(
                    pid_ctr.get_pid_current().map_err(|err| {
                        Error::Others(format!("failed to get current pid: {}", err))
                    })?,
                );
                pid_stats.set_limit(
                    pid_ctr
                        .get_pid_max()
                        .map(|val| match val {
                            // See https://github.com/opencontainers/runc/blob/dbe8434359ca35af1c1e10df42b1f4391c1e1010/libcontainer/cgroups/fs/pids.go#L55
                            cgroups_rs::MaxValue::Max => 0,
                            cgroups_rs::MaxValue::Value(val) => val as u64,
                        })
                        .map_err(|err| Error::Others(format!("failed to get max pid: {}", err)))?,
                );
                metrics.set_pids(pid_stats);
            }
            _ => {
                // TODO: add other subsystems
            }
        }
    }

    let metrics = convert_to_any(Box::new(metrics)).map_err(|e| Error::Others(e.to_string()))?;
    Ok(metrics)
}
