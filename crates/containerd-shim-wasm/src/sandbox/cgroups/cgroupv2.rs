use super::super::{Error, Result};
use super::RawFD;
use super::{
    ensure_write_file, find_cgroup_mounts, list_cgroup_controllers, new_mount_iter, safe_join,
    Cgroup, CgroupOptions, Version,
};
use nix::sys::statfs;
pub use oci_spec::runtime::LinuxResources as Resources;
use std::fs;
use std::ops::Not;
use std::path::PathBuf;

pub struct CgroupV2 {
    base: PathBuf,
    path: PathBuf,
}

impl CgroupV2 {
    pub fn new(base: PathBuf, path: PathBuf) -> Self {
        CgroupV2 { base, path }
    }

    fn get_file(&self, name: &str) -> Result<PathBuf> {
        safe_join(self.base.clone(), self.path.join(name))
    }
}

impl Cgroup for CgroupV2 {
    fn version(&self) -> Version {
        Version::V2
    }

    fn open(&self) -> Result<RawFD> {
        let path = safe_join(self.base.clone(), self.path.clone())?;
        fs::create_dir_all(&path)?;
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
        let path = safe_join(self.base.clone(), self.path.clone())?;
        if path.exists() {
            fs::remove_dir(path)?;
        }
        Ok(())
    }

    fn add_task(&self, pid: u32) -> Result<()> {
        ensure_write_file(self.get_file("cgroup.procs")?, &format!("{}", pid))?;
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
                ensure_write_file(self.get_file("cpu.weight")?, &format!("{}", s))?;
            }
            let mut max = "max".to_string();
            if let Some(quota) = cpu.quota() {
                max = format!("{}", quota);
            }
            if let Some(period) = cpu.period() {
                ensure_write_file(self.get_file("cpu.max")?, &format!("{} {}", max, period))?;
            } else {
                ensure_write_file(self.get_file("cpu.max")?, max.as_str())?;
            }

            // no realtime support
        }

        if let Some(mem) = res.memory() {
            if let Some(limit) = mem.limit() {
                ensure_write_file(self.get_file("memory.max")?, &format!("{}", limit))?;

                if let Some(swap) = mem.swap() {
                    // OCI spec expects swap to be memory+swap (because that's how cgroup v1 does things)
                    // V2 expects just the swap limit, so we need to subtract the swap limit (again, memory+swap) from the memory limit to get the swap total.
                    ensure_write_file(
                        self.get_file("memory.swap.max")?,
                        &format!("{}", limit - swap),
                    )?;
                }
            }

            if let Some(reservation) = mem.reservation() {
                ensure_write_file(self.get_file("memory.low")?, &format!("{}", reservation))?;
            }

            if let Some(oom_kill_disable) = mem.disable_oom_killer() {
                ensure_write_file(
                    self.get_file("memory.oom.group")?,
                    if oom_kill_disable { "1" } else { "0" },
                )?;
            }
        }

        if let Some(pids) = res.pids() {
            ensure_write_file(self.get_file("pids.max")?, &format!("{}", pids.limit()))?;
        }

        if let Some(hugepage) = res.hugepage_limits() {
            for limit in hugepage {
                let path = self.get_file(&format!("hugetlb.{}.max", limit.page_size()))?;
                ensure_write_file(path, &format!("{}", limit.limit()))?;
            }
        }

        if let Some(blkio) = res.block_io() {
            if let Some(weight) = blkio.weight() {
                ensure_write_file(
                    self.get_file("io.weight")?,
                    &format!("{}", 1 + (weight - 10) * 9999 / 990),
                )?;
            }

            if let Some(throttle_write_bps_device) = blkio.throttle_read_bps_device() {
                for device in throttle_write_bps_device {
                    let path = self.get_file("io.max")?;
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
                    let path = self.get_file("io.max")?;
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
                    let path = self.get_file("io.max")?;
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
                    let path = self.get_file("io.max")?;
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
                ensure_write_file(self.get_file(k)?, v)?;
            }
        }

        Ok(())
    }
}

impl TryFrom<&str> for CgroupV2 {
    type Error = Error;
    fn try_from(s: &str) -> Result<Self> {
        CgroupOptions {
            name: s.to_string(),
            root: None,
            mounts: new_mount_iter,
            controllers: None,
        }
        .try_into()
    }
}

impl<T: std::io::BufRead> TryFrom<CgroupOptions<T>> for CgroupV2 {
    type Error = Error;

    fn try_from(opts: CgroupOptions<T>) -> Result<Self> {
        Self::try_from(&opts)
    }
}

impl<T: std::io::BufRead> TryFrom<&CgroupOptions<T>> for CgroupV2 {
    type Error = Error;

    fn try_from(opts: &CgroupOptions<T>) -> Result<Self> {
        if let Some(root) = &opts.root {
            let stat = statfs::statfs(root)?;
            if stat.filesystem_type() != statfs::CGROUP2_SUPER_MAGIC {
                return Err(Error::InvalidArgument(format!(
                    "not a cgroup2 mount point: {}",
                    root.to_str().unwrap(),
                )));
            }
            return Ok(CgroupV2::new(
                PathBuf::from(root),
                PathBuf::from(&opts.name.clone()),
            ));
        }

        let f = fs::File::open("/proc/cgroups")?;
        let mounts = find_cgroup_mounts((&opts.mounts)()?, &list_cgroup_controllers(f)?)?;

        if let Some(mount) = mounts.v2 {
            if mounts.v1.is_empty().not() {
                return Err(Error::FailedPrecondition(
                    "cgroup v2 mount found but cgroup v1 mount also found: hybrid cgroup mode is not supported".to_string(),
                ));
            }
            return Ok(CgroupV2::new(
                PathBuf::from(mount),
                PathBuf::from(&opts.name.clone()),
            ));
        }

        Err(Error::FailedPrecondition(
            "cgroup2 mount not found".to_string(),
        ))
    }
}
