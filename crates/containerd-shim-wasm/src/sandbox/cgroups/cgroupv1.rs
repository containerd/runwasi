use super::super::{Error, Result};
use super::{
    ensure_write_file, find_cgroup_mounts, list_cgroup_controllers, new_mount_iter, safe_join,
    Cgroup, CgroupOptions, Version,
};
pub use oci_spec::runtime::{LinuxDeviceCgroup, LinuxDeviceType, LinuxResources as Resources};
use std::collections::HashMap;
use std::fs;
use std::io::prelude::Write;
use std::ops::Not;
use std::path::PathBuf;

pub struct CgroupV1 {
    path: PathBuf,
    controllers: HashMap<String, PathBuf>,
}

mod files {
    pub const CPU_SHARES: &str = "cpu.shares";
    pub const CPU_CFS_QUOTA: &str = "cpu.cfs_quota_us";
    pub const CPU_CFS_PERIOD: &str = "cpu.cfs_period_us";

    pub const CPU_RT_RUNTIME: &str = "cpu.rt_runtime_us";
    pub const CPU_RT_PERIOD: &str = "cpu.rt_period_us";

    pub const CPUSET_CPUS: &str = "cpuset.cpus";
    pub const CPUSET_MEMS: &str = "cpuset.mems";

    pub const DEVICES_ALLOW: &str = "devices.allow";
    pub const DEVICES_DENY: &str = "devices.deny";

    pub const MEMORY_HARD_LIMIT: &str = "memory.limit_in_bytes";
    pub const MEMORY_SOFT_LIMIT: &str = "memory.soft_limit_in_bytes";
    pub const MEMORY_SWAP_LIMIT: &str = "memory.memsw.limit_in_bytes";
    pub const MEMORY_SWAPPINESS: &str = "memory.swappiness";
    pub const MEMORY_KMEM_LIMIT: &str = "memory.kmem.limit_in_bytes";
    pub const MEMORY_KMEM_TCP_LIMIT: &str = "memory.kmem.tcp.limit_in_bytes";
    pub const MEMORY_OOM_CONTROL: &str = "memory.oom_control";

    pub const PIDS_MAX: &str = "pids.max";

    pub const BLKIO_WEIGHT: &str = "blkio.weight";
    pub const BLKIO_WEIGHT_DEVICE: &str = "blkio.weight_device";
    pub const BLKIO_READ_BPS: &str = "blkio.throttle.read_bps_device";
    pub const BLKIO_WRITE_BPS: &str = "blkio.throttle.write_bps_device";
    pub const BLKIO_READ_IOPS: &str = "blkio.throttle.read_iops_device";
    pub const BLKIO_WRITE_IOPS: &str = "blkio.throttle.write_iops_device";

    pub const NET_CLS_CLASSID: &str = "net_cls.classid";
    pub const NET_CLS_PRIORITY: &str = "net_prio.ifpriomap";

    pub const RDMA_MAX: &str = "rdma.max";
}

use files::*;

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
        (&self.controllers).into_iter().try_for_each(|(kind, _)| {
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
                ensure_write_file(controller_path.join(CPU_CFS_QUOTA), &quota.to_string())?;
            }
            if let Some(period) = cpu.period() {
                ensure_write_file(controller_path.join(CPU_CFS_PERIOD), &period.to_string())?;
            }
            if let Some(shares) = cpu.shares() {
                ensure_write_file(controller_path.join(CPU_SHARES), &shares.to_string())?;
            }
            if let Some(realtime_period) = cpu.realtime_period() {
                ensure_write_file(
                    controller_path.join(CPU_RT_PERIOD),
                    &realtime_period.to_string(),
                )?;
            }
            if let Some(realtime_runtime) = cpu.realtime_runtime() {
                ensure_write_file(
                    controller_path.join(CPU_RT_RUNTIME),
                    &realtime_runtime.to_string(),
                )?;
            }

            let cpuset_path = self.get_controller("cpuset")?;
            if let Some(cpus) = cpu.cpus() {
                ensure_write_file(cpuset_path.join(CPUSET_CPUS), cpus)?;
            }
            if let Some(mems) = cpu.mems() {
                ensure_write_file(cpuset_path.join(CPUSET_MEMS), mems)?;
            }
        }

        if let Some(memory) = res.memory() {
            let mut mem_unlimited = false;
            let controller_path = self.get_controller("memory")?;
            if let Some(limit) = memory.limit() {
                if limit == -1 {
                    mem_unlimited = true;
                }
                ensure_write_file(controller_path.join(MEMORY_HARD_LIMIT), &limit.to_string())?;
            }
            match memory.swap() {
                Some(limit) => {
                    ensure_write_file(controller_path.join(MEMORY_SWAP_LIMIT), &limit.to_string())?;
                }
                None => {
                    // If memory is unlimited and swap is not explicitly set, set swap to unlimited
                    // See https://github.com/opencontainers/runc/blob/eddf35e5462e2a9f24d8279874a84cfc8b8453c2/libcontainer/cgroups/fs/memory.go#L70-L71
                    if mem_unlimited {
                        ensure_write_file(controller_path.join(MEMORY_SWAP_LIMIT), "-1")?;
                    }
                }
            }

            if let Some(reservation) = memory.reservation() {
                ensure_write_file(
                    controller_path.join(MEMORY_SOFT_LIMIT),
                    &reservation.to_string(),
                )?;
            }
            if let Some(swappiness) = memory.swappiness() {
                ensure_write_file(
                    controller_path.join(MEMORY_SWAPPINESS),
                    &swappiness.to_string(),
                )?;
            }
            if let Some(kernel) = memory.kernel() {
                ensure_write_file(controller_path.join(MEMORY_KMEM_LIMIT), &kernel.to_string())?;
            }
            if let Some(kernel_tcp) = memory.kernel_tcp() {
                ensure_write_file(
                    controller_path.join(MEMORY_KMEM_TCP_LIMIT),
                    &kernel_tcp.to_string(),
                )?;
            }
            if let Some(oom_kill_disable) = memory.disable_oom_killer() {
                if oom_kill_disable {
                    ensure_write_file(controller_path.join(MEMORY_OOM_CONTROL), "1")?;
                }
            }
        }

        if let Some(pids) = &res.pids() {
            let controller_path = self.get_controller("pids")?;
            ensure_write_file(controller_path.join(PIDS_MAX), &pids.limit().to_string())?;
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
                ensure_write_file(controller_path.join(BLKIO_WEIGHT), &weight.to_string())?;
            }
            if let Some(weight_device) = blkio.weight_device() {
                let mut file = self.open_file("blkio", BLKIO_WEIGHT_DEVICE)?;
                for device in weight_device {
                    if let Some(weight) = device.weight() {
                        file.write_all(
                            format!("{}:{} {}", device.major(), device.minor(), weight).as_bytes(),
                        )?;
                    }
                }
            }
            if let Some(throttle_read_bps_device) = blkio.throttle_read_bps_device() {
                let mut file = self.open_file("blkio", BLKIO_READ_BPS)?;
                for device in throttle_read_bps_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_write_bps_device) = blkio.throttle_write_bps_device() {
                let mut file = self.open_file("blkio", BLKIO_WRITE_BPS)?;
                for device in throttle_write_bps_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_read_iops_device) = blkio.throttle_read_iops_device() {
                let mut file = self.open_file("blkio", BLKIO_READ_IOPS)?;
                for device in throttle_read_iops_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_write_iops_device) = blkio.throttle_write_iops_device() {
                let mut file = self.open_file("blkio", BLKIO_WRITE_IOPS)?;
                for device in throttle_write_iops_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
        }

        if let Some(net_cls) = res.network() {
            let controller_path = self.get_controller("net_cls")?;
            if let Some(classid) = net_cls.class_id() {
                ensure_write_file(
                    controller_path.join(NET_CLS_CLASSID),
                    classid.to_string().as_str(),
                )?;
            }

            if let Some(priorities) = net_cls.priorities() {
                let mut file = self.open_file("net_cls", NET_CLS_PRIORITY)?;
                for priority in priorities {
                    file.write_all(
                        format!("{} {}", priority.name(), priority.priority()).as_bytes(),
                    )?;
                }
            }
        }

        if let Some(devices) = res.devices() {
            let mut allow = self.open_file("devices", DEVICES_ALLOW)?;
            let mut deny = self.open_file("devices", DEVICES_DENY)?;
            for device in devices {
                let formatted = format_device(device);
                if device.allow() {
                    allow.write_all(formatted.as_bytes()).map_err(|e| {
                        Error::Others(format!(
                            "error writing to devices.allow: {}: {}",
                            e, formatted
                        ))
                    })?;
                } else {
                    deny.write_all(formatted.as_bytes()).map_err(|e| {
                        Error::Others(format!(
                            "error writing to devices.deny: {}: {}",
                            e, formatted
                        ))
                    })?;
                }
            }
        }

        if let Some(rdma) = res.rdma() {
            let mut max = self.open_file("rdma", RDMA_MAX)?;
            rdma.iter().try_for_each(|(device, limits)| {
                let mut s = String::new();
                if let Some(hca_handle) = limits.hca_handles() {
                    s.push_str(&format!("hca_handle={} ", hca_handle));
                }
                if let Some(hca_object) = limits.hca_objects() {
                    s.push_str(&format!("hca_object={} ", hca_object));
                }
                if s.is_empty().not() {
                    return max.write_all(format!("{} {}", device, s).as_bytes());
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

fn format_device(device: &LinuxDeviceCgroup) -> String {
    let mut s = String::new();
    let typ = device.typ().unwrap_or(LinuxDeviceType::A);
    s.push_str(typ.as_str());
    s.push(' ');

    match device.major() {
        Some(major) => s.push_str(&format!("{}:", major)),
        None => s.push_str("*:"),
    }

    match device.minor() {
        Some(minor) => s.push_str(&minor.to_string()),
        None => s.push_str("*"),
    }
    s.push(' ');

    if let Some(access) = device.access() {
        s.push_str(access.as_str());
    }
    s
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
