use super::super::{Error, Result};
use super::{
    ensure_write_file, find_cgroup_mounts, list_cgroup_controllers, new_mount_iter, safe_join,
    Cgroup, CgroupOptions, Version,
};
pub use oci_spec::runtime::LinuxResources as Resources;
use std::collections::HashMap;
use std::fs;
use std::io::prelude::Write;
use std::ops::Not;
use std::path::PathBuf;

pub struct CgroupV1 {
    path: PathBuf,
    controllers: HashMap<String, PathBuf>,
}

impl CgroupV1 {
    fn open_file(&self, ctrl: &str, file: &str) -> Result<fs::File> {
        let path = self.get_controller(ctrl)?.join(file);
        fs::OpenOptions::new().write(true).open(&path).map_err(|e| {
            Error::Others(format!(
                "failed to open cgroup file {}: {}",
                path.display(),
                e
            ))
        })
    }

    fn get_controller(&self, controller: &str) -> Result<PathBuf> {
        let controller_path = self.controllers.get(controller).cloned().ok_or_else(|| {
            Error::FailedPrecondition(format!(
                "controller {} is not enabled for cgroup {}",
                controller,
                self.path.display()
            ))
        })?;

        if !controller_path.exists() {
            return Err(Error::Others(format!(
                "controller not found {}: {}",
                controller,
                controller_path.to_str().unwrap(),
            )));
        }

        let p = safe_join(controller_path, self.path.clone())?;
        if !p.exists() {
            fs::create_dir_all(&p).map_err(|e| {
                Error::Others(format!("error creating cgroup path {}: {}", p.display(), e))
            })?;
        }
        Ok(p)
    }
}

impl Cgroup for CgroupV1 {
    fn version(&self) -> Version {
        Version::V1
    }

    fn delete(&self) -> Result<()> {
        (&self.controllers)
            .into_iter()
            .try_for_each(|(kind, _subsys)| {
                let dir = self.get_controller(kind)?;
                if dir.exists() {
                    return fs::remove_dir(&dir).map_err(|e| {
                        Error::Others(format!("error deleting cgroup {}: {}", dir.display(), e))
                    });
                }
                Ok(())
            })
    }

    fn add_task(&self, pid: u32) -> Result<()> {
        // cpuset is special, we can't add a process to it unless values are already initialized
        let cpuset = self.get_controller("cpuset")?;
        if let Ok(v) = fs::read_to_string(cpuset.join("cpuset.cpus")) {
            if v.trim().is_empty().not() {
                ensure_write_file(cpuset.join("cgroup.procs"), &format!("{}", pid))?;
            };
        }

        self.controllers
            .iter()
            .map(|(kind, _subsys)| {
                if kind == "cpuset" {
                    return Ok(());
                }
                ensure_write_file(
                    self.get_controller(kind)?.join("cgroup.procs"),
                    &format!("{}", pid),
                )
            })
            .collect::<Result<Vec<()>>>()?;
        Ok(())
    }

    fn apply(&self, res: Option<Resources>) -> Result<()> {
        let res = match res {
            Some(r) => r,
            None => return Ok(()),
        };

        if let Some(cpu) = res.cpu() {
            let controller_path = self.get_controller("cpu")?;
            if let Some(quota) = cpu.quota() {
                ensure_write_file(controller_path.join("cpu.cfs_quota_us"), &quota.to_string())?;
            }
            if let Some(period) = cpu.period() {
                ensure_write_file(
                    controller_path.join("cpu.cfs_period_us"),
                    &period.to_string(),
                )?;
            }
            if let Some(shares) = cpu.shares() {
                ensure_write_file(controller_path.join("cpu.shares"), &shares.to_string())?;
            }
            if let Some(realtime_period) = cpu.realtime_period() {
                ensure_write_file(
                    controller_path.join("cpu.rt_period_us"),
                    &realtime_period.to_string(),
                )?;
            }
            if let Some(realtime_runtime) = cpu.realtime_runtime() {
                ensure_write_file(
                    controller_path.join("cpu.rt_runtime_us"),
                    &realtime_runtime.to_string(),
                )?;
            }

            let cpuset_path = self.get_controller("cpuset")?;
            if let Some(cpus) = cpu.cpus() {
                ensure_write_file(cpuset_path.join("cpuset.cpus"), cpus)?;
            }
            if let Some(mems) = cpu.mems() {
                ensure_write_file(cpuset_path.join("cpuset.mems"), mems)?;
            }
        }

        if let Some(memory) = res.memory() {
            let mut mem_unlimited = false;
            let controller_path = self.get_controller("memory")?;
            if let Some(limit) = memory.limit() {
                if limit == -1 {
                    mem_unlimited = true;
                }
                ensure_write_file(
                    controller_path.join("memory.limit_in_bytes"),
                    &limit.to_string(),
                )?;
            }
            if let Some(swap) = memory.swap() {
                ensure_write_file(
                    controller_path.join("memory.memsw.limit_in_bytes"),
                    &swap.to_string(),
                )?;
            } else {
                // If memory is unlimited and swap is not explicitly set, set swap to unlimited
                // See https://github.com/opencontainers/runc/blob/eddf35e5462e2a9f24d8279874a84cfc8b8453c2/libcontainer/cgroups/fs/memory.go#L70-L71
                if mem_unlimited {
                    ensure_write_file(controller_path.join("memory.memsw.limit_in_bytes"), "-1")?;
                }
            }
            if let Some(reservation) = memory.reservation() {
                ensure_write_file(
                    controller_path.join("memory.soft_limit_in_bytes"),
                    &reservation.to_string(),
                )?;
            }
            if let Some(swappiness) = memory.swappiness() {
                ensure_write_file(
                    controller_path.join("memory.swappiness"),
                    &swappiness.to_string(),
                )?;
            }
            if let Some(kernel) = memory.kernel() {
                ensure_write_file(
                    controller_path.join("memory.kmem.limit_in_bytes"),
                    &kernel.to_string(),
                )?;
            }
            if let Some(kernel_tcp) = memory.kernel_tcp() {
                ensure_write_file(
                    controller_path.join("memory.kmem.tcp.limit_in_bytes"),
                    &kernel_tcp.to_string(),
                )?;
            }
            if let Some(oom_kill_disable) = memory.disable_oom_killer() {
                if oom_kill_disable {
                    ensure_write_file(controller_path.join("memory.oom_control"), "1")?;
                }
            }
        }

        if let Some(pids) = &res.pids() {
            let controller_path = self.get_controller("pids")?;
            ensure_write_file(controller_path.join("pids.max"), &pids.limit().to_string())?;
        }

        if let Some(hugepages) = res.hugepage_limits() {
            for page in hugepages {
                let controller_path = self.get_controller("hugetlb")?;
                let path =
                    controller_path.join(format!("hugetlb.{}.limit_in_bytes", page.page_size()));
                ensure_write_file(path, &page.limit().to_string())?;
            }
        }

        if let Some(blkio) = res.block_io() {
            let controller_path = self.get_controller("blkio")?;
            if let Some(weight) = blkio.weight() {
                ensure_write_file(controller_path.join("blkio.weight"), &weight.to_string())?;
            }
            if let Some(weight_device) = blkio.weight_device() {
                let mut file = self.open_file("blkio", "blockio.weight_device")?;
                for device in weight_device {
                    if let Some(weight) = device.weight() {
                        file.write_all(
                            format!("{}:{} {}", device.major(), device.minor(), weight).as_bytes(),
                        )?;
                    }
                }
            }
            if let Some(throttle_read_bps_device) = blkio.throttle_read_bps_device() {
                let mut file = self.open_file("blkio", "blkio.throttle.read_bps_device")?;
                for device in throttle_read_bps_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_write_bps_device) = blkio.throttle_write_bps_device() {
                let mut file = self.open_file("blkio", "blkio.throttle.write_bps_device")?;
                for device in throttle_write_bps_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_read_iops_device) = blkio.throttle_read_iops_device() {
                let mut file = self.open_file("blkio", "blkio.throttle.read_iops_device")?;
                for device in throttle_read_iops_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_write_iops_device) = blkio.throttle_write_iops_device() {
                let mut file = self.open_file("blkio", "blkio.throttle.write_iops_device")?;
                for device in throttle_write_iops_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl TryFrom<&str> for CgroupV1 {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self> {
        let opts: CgroupOptions<std::io::BufReader<fs::File>> = CgroupOptions {
            name: s.to_string(),
            root: None,
            mounts: new_mount_iter,
            controllers: None,
        };
        Self::try_from(&opts)
    }
}

impl<T: std::io::BufRead> TryFrom<CgroupOptions<T>> for CgroupV1 {
    type Error = Error;
    fn try_from(opts: CgroupOptions<T>) -> Result<Self> {
        Self::try_from(&opts)
    }
}

impl<T: std::io::BufRead> TryFrom<&CgroupOptions<T>> for CgroupV1 {
    type Error = Error;

    fn try_from(opts: &CgroupOptions<T>) -> Result<Self> {
        let controllers = match &opts.controllers {
            Some(controllers) => controllers.clone(),
            None => {
                let mounts = find_cgroup_mounts(
                    (opts.mounts)()?,
                    &list_cgroup_controllers(fs::File::open("/proc/cgroups")?)?,
                )?;
                if let Some(v2) = mounts.v2 {
                    if fs::read_to_string(v2.join("cgroup.controllers"))?
                        .trim()
                        .is_empty()
                        .not()
                    {
                        return Err(Error::FailedPrecondition(format!(
                            "found cgroup2 mount at {}: hybrid cgroup v1/v2 is not supported",
                            v2.display()
                        )));
                    }
                }
                mounts.v1
            }
        };

        if controllers.is_empty() {
            return Err(Error::FailedPrecondition(
                "no cgroup v1 controllers found".to_string(),
            ));
        }

        Ok(Self {
            path: PathBuf::from(opts.name.clone()),
            controllers,
        })
    }
}
