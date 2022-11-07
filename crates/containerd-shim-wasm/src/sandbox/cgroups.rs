use super::{Error, Result};
use nix::sys::statfs::statfs;
pub use oci_spec::runtime::LinuxResources as Resources;
use proc_mounts::MountIter;
use std::fs::{create_dir_all, OpenOptions};
use std::io::prelude::Write;
use std::os::raw::c_int as RawFD;
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub enum Version {
    V1,
    V2,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = match self {
            Version::V1 => "v1",
            Version::V2 => "v2",
        };

        write!(f, "{}", v)
    }
}

pub trait Cgroup {
    fn version(&self) -> Version;

    // open is optional, currently only used by v2 for use with clone3
    // clone3 doesn't support cgroupv1 so we don't need to implement it.
    fn open(&self) -> Result<RawFD> {
        Err(Error::Others(format!(
            "open not implemented for cgroup version: {}",
            self.version().to_string()
        )))
    }

    fn apply(&self, _res: Option<Resources>) -> Result<()> {
        Err(Error::Others(format!(
            "cgroup {} is not supported",
            self.version().to_string(),
        )))
    }

    fn add_task(&self, pid: u32) -> Result<()>;

    fn delete(&self) -> Result<()>;
}

pub struct CgroupV1 {
    path: PathBuf,
    base: PathBuf,
}

impl Cgroup for CgroupV1 {
    fn version(&self) -> Version {
        Version::V1
    }

    fn delete(&self) -> Result<()> {
        vec!["cpu", "cpuset", "memory", "pids"]
            .iter()
            .try_for_each(|subsys| {
                let path = self.base.join(subsys).join(&self.path);
                if path.exists() {
                    std::fs::remove_dir(path)?;
                }
                Ok(())
            })
    }

    fn add_task(&self, pid: u32) -> Result<()> {
        // cpuset is special, we can't add a process to it unless values are already initialized
        let cpuset = self.get_controller("cpuset")?;
        if let Ok(v) = std::fs::read_to_string(cpuset.join("cpuset.cpus")) {
            if !v.trim().is_empty() {
                ensure_write_file(cpuset.join("cgroup.procs"), &format!("{}", pid))?;
            }
        }

        vec!["cpu", "memory", "pids"]
            .iter()
            .map(|subsys| {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(self.get_controller(subsys)?.join("cgroup.procs"))?;
                file.write_all(pid.to_string().as_bytes())?;
                Ok(())
            })
            .collect::<Result<Vec<()>>>()?;
        Ok(())
    }

    fn apply(&self, res: Option<Resources>) -> Result<()> {
        if res.is_none() {
            return Ok(());
        }
        let res = res.unwrap();

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
        let mut mem_unlimited = false;
        if let Some(memory) = res.memory() {
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
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(controller_path.join("blkio.weight_device"))?;
                for device in weight_device {
                    if let Some(weight) = device.weight() {
                        file.write_all(
                            format!("{}:{} {}", device.major(), device.minor(), weight).as_bytes(),
                        )?;
                    }
                }
            }
            if let Some(throttle_read_bps_device) = blkio.throttle_read_bps_device() {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(controller_path.join("blkio.throttle.read_bps_device"))?;
                for device in throttle_read_bps_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_write_bps_device) = blkio.throttle_write_bps_device() {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(controller_path.join("blkio.throttle.write_bps_device"))?;
                for device in throttle_write_bps_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_read_iops_device) = blkio.throttle_read_iops_device() {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(controller_path.join("blkio.throttle.read_iops_device"))?;
                for device in throttle_read_iops_device {
                    file.write_all(
                        format!("{}:{} {}", device.major(), device.minor(), device.rate())
                            .as_bytes(),
                    )?;
                }
            }
            if let Some(throttle_write_iops_device) = blkio.throttle_write_iops_device() {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(controller_path.join("blkio.throttle.write_iops_device"))?;
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

impl CgroupV1 {
    pub fn new(base: PathBuf, path: PathBuf) -> Self {
        CgroupV1 { path, base }
    }

    fn get_controller(&self, controller: &str) -> Result<PathBuf> {
        let controller_path = self.base.join(controller);
        if !controller_path.exists() {
            return Err(Error::Others(format!(
                "controller not found {}: {}",
                controller,
                controller_path.to_str().unwrap(),
            )));
        }

        let p = controller_path.join(&self.path);
        if !p.exists() {
            std::fs::create_dir_all(&p)?;
        }
        Ok(p)
    }
}

pub struct CgroupV2 {
    base: PathBuf,
    path: PathBuf,
}

impl CgroupV2 {
    pub fn new(base: PathBuf, path: PathBuf) -> Self {
        CgroupV2 { base, path }
    }

    fn get_file(&self, name: &str) -> PathBuf {
        self.base.join(&self.path).join(name)
    }
}

fn ensure_write_file(path: std::path::PathBuf, content: &str) -> Result<()> {
    let parent = path.parent().unwrap();
    if !parent.exists() {
        create_dir_all(parent).map_err(|e| {
            Error::Others(format!(
                "could not create parent cgroup dir {}: {}",
                parent.to_str().unwrap(),
                e
            ))
        })?;
    }
    let mut file = OpenOptions::new().write(true).open(&path).map_err(|e| {
        Error::Others(format!(
            "could not open cgroup file {}: {}",
            path.to_str().unwrap(),
            e
        ))
    })?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

impl Cgroup for CgroupV2 {
    fn version(&self) -> Version {
        Version::V2
    }

    fn open(&self) -> Result<RawFD> {
        let path = self.base.join(&self.path);
        create_dir_all(&path)?;
        nix::fcntl::open(
            path.as_path(),
            nix::fcntl::OFlag::O_DIRECTORY
                | nix::fcntl::OFlag::O_CLOEXEC
                | nix::fcntl::OFlag::O_RDONLY,
            nix::sys::stat::Mode::empty(),
        )
        .map_err(|e| std::io::Error::from(e).into())
    }

    fn delete(&self) -> Result<()> {
        let path = self.base.join(&self.path);
        if path.exists() {
            std::fs::remove_dir(path)?;
        }
        Ok(())
    }

    fn add_task(&self, pid: u32) -> Result<()> {
        ensure_write_file(self.get_file("cgroup.procs"), &format!("{}", pid))?;
        Ok(())
    }

    // See https://github.com/containerd/cgroups/blob/724eb82fe759f3b3b9c5f07d22d2fab93467dc56/v2/utils.go#L164
    // for details on converting the oci spec (which is heavily v1 focussed) to v2.
    // Also https://github.com/containers/crun/blob/2497b9bb03623838d37a1587087f1ad3d6ff28ec/crun.1.md#cgroup-v2
    fn apply(&self, res: Option<Resources>) -> Result<()> {
        if res.is_none() {
            return Ok(());
        }
        let res = res.unwrap();

        if let Some(cpu) = res.cpu() {
            if let Some(shares) = cpu.shares() {
                let s = 1 + ((shares - 2) * 9999) / 262142;
                ensure_write_file(self.get_file("cpu.weight"), &format!("{}", s))?;
            }
            let mut max = "max".to_string();
            if let Some(quota) = cpu.quota() {
                max = format!("{}", quota);
            }
            if let Some(period) = cpu.period() {
                ensure_write_file(self.get_file("cpu.max"), &format!("{} {}", max, period))?;
            } else {
                ensure_write_file(self.get_file("cpu.max"), max.as_str())?;
            }

            // no realtime support
        }

        if let Some(mem) = res.memory() {
            if let Some(limit) = mem.limit() {
                ensure_write_file(self.get_file("memory.max"), &format!("{}", limit))?;

                if let Some(swap) = mem.swap() {
                    // OCI spec expects swap to be memory+swap (because that's how cgroup v1 does things)
                    // V2 expects just the swap limit, so we need to subtract the swap limit (again, memory+swap) from the memory limit to get the swap total.
                    ensure_write_file(
                        self.get_file("memory.swap.max"),
                        &format!("{}", limit - swap),
                    )?;
                }
            }

            if let Some(reservation) = mem.reservation() {
                ensure_write_file(self.get_file("memory.low"), &format!("{}", reservation))?;
            }

            if let Some(oom_kill_disable) = mem.disable_oom_killer() {
                ensure_write_file(
                    self.get_file("memory.oom.group"),
                    if oom_kill_disable { "1" } else { "0" },
                )?;
            }
        }

        if let Some(pids) = res.pids() {
            ensure_write_file(self.get_file("pids.max"), &format!("{}", pids.limit()))?;
        }

        if let Some(hugepage) = res.hugepage_limits() {
            for limit in hugepage {
                let path = self.get_file(&format!("hugetlb.{}.max", limit.page_size()));
                ensure_write_file(path, &format!("{}", limit.limit()))?;
            }
        }

        if let Some(blkio) = res.block_io() {
            if let Some(weight) = blkio.weight() {
                ensure_write_file(
                    self.get_file("io.weight"),
                    &format!("{}", 1 + (weight - 10) * 9999 / 990),
                )?;
            }

            if let Some(throttle_write_bps_device) = blkio.throttle_read_bps_device() {
                for device in throttle_write_bps_device {
                    let path = self.get_file("io.max");
                    ensure_write_file(
                        path,
                        &format!(
                            "{}:{} rbps={}",
                            device.major(),
                            device.minor(),
                            device.rate()
                        ),
                    )?;
                }
            }

            if let Some(throttle_write_bps_device) = blkio.throttle_write_bps_device() {
                for device in throttle_write_bps_device {
                    let path = self.get_file("io.max");
                    ensure_write_file(
                        path,
                        &format!(
                            "{}:{} wbps={}",
                            device.major(),
                            device.minor(),
                            device.rate()
                        ),
                    )?;
                }
            }

            if let Some(throttle_write_bps_device) = blkio.throttle_read_iops_device() {
                for device in throttle_write_bps_device {
                    let path = self.get_file("io.max");
                    ensure_write_file(
                        path,
                        &format!(
                            "{}:{} riops={}",
                            device.major(),
                            device.minor(),
                            device.rate()
                        ),
                    )?;
                }
            }

            if let Some(throttle_write_bps_device) = blkio.throttle_write_iops_device() {
                for device in throttle_write_bps_device {
                    let path = self.get_file("io.max");
                    ensure_write_file(
                        path,
                        &format!(
                            "{}:{} wiops={}",
                            device.major(),
                            device.minor(),
                            device.rate()
                        ),
                    )?;
                }
            }
        }

        if let Some(unified) = res.unified() {
            for (k, v) in unified {
                ensure_write_file(self.get_file(k), v)?;
            }
        }

        Ok(())
    }
}

pub fn new(name: String) -> Result<Box<dyn Cgroup>> {
    // fast path
    let v1 = PathBuf::from("/sys/fs/cgroup");
    let v2 = PathBuf::from("/sys/fs/cgroup/unified");
    if v1.join("cpu").exists() {
        let stat = statfs(v1.join("cpu").as_path()).map_err(std::io::Error::from)?;
        let is_v1 = stat.filesystem_type() == nix::sys::statfs::CGROUP_SUPER_MAGIC;
        if is_v1 {
            if v2.exists() {
                // Check if we are running in a hybrid cgroup setup
                let data = std::fs::read(v2.join("cgroup.controllers"))?;

                let trimmed = std::str::from_utf8(&data)
                    .map_err(|e| {
                        Error::Others(format!(
                            "could not convert cgroup.controllers to string: {}",
                            e
                        ))
                    })?
                    .trim();

                if !trimmed.is_empty() {
                    return Err(Error::FailedPrecondition(
                        "hybyrid cgroup is not supported".to_string(),
                    ));
                }

                return Ok(Box::new(CgroupV1 {
                    base: v1,
                    path: PathBuf::from(name),
                }));
            }
            return Ok(Box::new(CgroupV1::new(v1, PathBuf::from(name))));
        }
    }

    if v2.exists() {
        return Ok(Box::new(CgroupV2::new(v2, PathBuf::from(name))));
    }

    if v1.exists() {
        let stat = nix::sys::statfs::statfs(v1.as_path()).map_err(std::io::Error::from)?;
        if stat.filesystem_type() == nix::sys::statfs::CGROUP2_SUPER_MAGIC {
            // cgroup2 is mounted directly on /sys/fs/cgroup
            return Ok(Box::new(CgroupV2::new(v1, PathBuf::from(name))));
        }
    }

    // slow path

    let mut v1_found = false;
    let mut v2_found = false;
    let mut base = PathBuf::from("/");

    // It's possible for V1 controllers to be mounted all over the place... this code does not support that.
    // It is expected that all v1 controllers are mounted under the same path
    for mount in MountIter::new()? {
        let mount = mount?;
        match mount.fstype.as_str() {
            "cgroup" => {
                base = mount.dest.parent().unwrap().to_path_buf();
                v1_found = true;
            }
            "cgroup2" => {
                base = mount.dest;
                v2_found = true;
            }
            _ => {}
        }
        if v1_found && v2_found {
            break;
        }
    }

    if v1_found && v2_found {
        let p = base.clone().join("cgroup.controllers");
        if !std::fs::read(p)?.is_empty() {
            return Ok(Box::new(CgroupV1 {
                base,
                path: PathBuf::from(name),
            }));
        }
        return Err(Error::FailedPrecondition(
            "hybyrid cgroup is not supported".to_string(),
        ));
    }

    if v1_found {
        return Ok(Box::new(CgroupV1::new(base, PathBuf::from(name))));
    }

    if v2_found {
        return Ok(Box::new(CgroupV2::new(base, PathBuf::from(name))));
    }

    Err(Error::FailedPrecondition(
        "could not detect cgroup version".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::info;
    use oci_spec::runtime::Spec;
    use serde_json as json;

    fn cgroup_test(cg: Box<&dyn Cgroup>) -> Result<()> {
        let s: Spec = json::from_str(
            r#"
        {
            "linux": {
                "resources": {
                    "memory": {
                        "limit": 1000000,
                        "reservation": 100000,
                        "swap": 1000000,
                        "kernel": 100000,
                        "kernelTCP": 100000,
                        "swappiness": 100,
                        "disableOOMKiller": true
                    }
                }
            }
        }"#,
        )?;
        cg.apply(Some(
            s.linux()
                .as_ref()
                .unwrap()
                .resources()
                .as_ref()
                .unwrap()
                .clone(),
        ))
        .map_err(|e| Error::Others(format!("failed to apply cgroup: {}", e)))
    }

    #[test]
    fn test_cgroup() -> Result<()> {
        if !super::super::exec::has_cap_sys_admin() {
            info!("skipping cgroup test because we don't have CAP_SYS_ADMIN");
            return Ok(());
        }
        let cg = new("containerd-wasm-shim-test_cgroup".to_string())?;
        let res = cgroup_test(Box::new(&*cg));
        if cg.version() == Version::V2 {
            cg.open()?;
        }
        cg.delete()?;
        res
    }
}
