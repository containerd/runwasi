use super::{Error, Result};
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
        write!(f, "{}", self.to_string().as_str())
    }
}

impl Version {
    pub fn to_string(&self) -> String {
        match self {
            Version::V1 => "v1".to_string(),
            Version::V2 => "v2".to_string(),
        }
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
        vec!["cpu", "cpuset", "memory", "pids"]
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
        if let Some(res) = res {
            if let Some(cpu) = res.cpu() {
                let controller_path = self.get_controller("cpu")?;
                if let Some(quota) = cpu.quota() {
                    let path = controller_path.join("cpu.cfs_quota_us");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", quota).as_bytes())?
                }
                if let Some(period) = cpu.period() {
                    let path = controller_path.join("cpu.cfs_period_us");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", period).as_bytes())?
                }
                if let Some(shares) = cpu.shares() {
                    let path = controller_path.join("cpu.shares");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", shares).as_bytes())?
                }
                if let Some(realtime_period) = cpu.realtime_period() {
                    let path = controller_path.join("cpu.rt_period_us");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", realtime_period).as_bytes())?
                }
                if let Some(realtime_runtime) = cpu.realtime_runtime() {
                    let path = controller_path.join("cpu.rt_runtime_us");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", realtime_runtime).as_bytes())?
                }

                let cpuset_path = self.get_controller("cpuset")?;
                if let Some(cpus) = cpu.cpus() {
                    let path = cpuset_path.join("cpuset.cpus");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(cpus.as_bytes())?
                } else {
                    let path = cpuset_path.join("cpuset.cpus");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all("0".as_bytes())?
                }

                if let Some(mems) = cpu.mems() {
                    let path = cpuset_path.join("cpuset.mems");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(mems.as_bytes())?
                } else {
                    let path = cpuset_path.join("cpuset.mems");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all("0".as_bytes())?
                }
            }
            if let Some(memory) = &res.memory() {
                let controller_path = self.get_controller("memory")?;
                if let Some(limit) = memory.limit() {
                    let path = controller_path.join("memory.limit_in_bytes");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", limit).as_bytes())?;
                }
                if let Some(reservation) = memory.reservation() {
                    let path = controller_path.join("memory.soft_limit_in_bytes");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", reservation).as_bytes())?;
                }
                if let Some(swappiness) = memory.swappiness() {
                    let path = controller_path.join("memory.swappiness");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", swappiness).as_bytes())?;
                }
                if let Some(kernel) = memory.kernel() {
                    let path = controller_path.join("memory.kmem.limit_in_bytes");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", kernel).as_bytes())?;
                }
                if let Some(kernel_tcp) = memory.kernel_tcp() {
                    let path = controller_path.join("memory.kmem.tcp.limit_in_bytes");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", kernel_tcp).as_bytes())?;
                }
                if let Some(swap) = memory.swap() {
                    let path = controller_path.join("memory.memsw.limit_in_bytes");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(format!("{}", swap).as_bytes())?;
                }
                if let Some(oom_kill_disable) = memory.disable_oom_killer() {
                    let path = controller_path.join("memory.oom_control");
                    let mut file = OpenOptions::new().write(true).open(path)?;
                    file.write_all(if oom_kill_disable { b"1" } else { b"0" })?;
                }
            }

            if let Some(pids) = &res.pids() {
                let controller_path = self.get_controller("pids")?;
                let limit = pids.limit();
                let path = controller_path.join("pids.max");
                let mut file = OpenOptions::new().write(true).open(path)?;
                file.write_all(format!("{}", limit).as_bytes())?;
            }
        }
        Ok(())
    }
}

impl CgroupV1 {
    pub fn new(base: PathBuf, path: PathBuf) -> Self {
        CgroupV1 {
            path: path,
            base: base,
        }
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
        CgroupV2 {
            base: base,
            path: path,
        }
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

    fn apply(&self, res: Option<Resources>) -> Result<()> {
        if let Some(res) = res {
            if let Some(cpu) = res.cpu() {
                if let Some(shares) = cpu.shares() {
                    ensure_write_file(self.get_file("cpu.weight"), &format!("{}", shares))?;
                }
                if let Some(quota) = cpu.quota() {
                    ensure_write_file(self.get_file("cpu.max"), &format!("{}", quota))?;
                }
                if let Some(realtime_runtime) = cpu.realtime_runtime() {
                    ensure_write_file(
                        self.get_file("cpu.rt_runtime_us"),
                        &format!("{}", realtime_runtime),
                    )?;
                }
                if let Some(realtime_period) = cpu.realtime_period() {
                    ensure_write_file(
                        self.get_file("cpu.rt_period_us"),
                        &format!("{}", realtime_period),
                    )?;
                }
            }

            if let Some(mem) = res.memory() {
                if let Some(limit) = mem.limit() {
                    ensure_write_file(self.get_file("memory.max"), &format!("{}", limit))?;
                }
                if let Some(reservation) = mem.reservation() {
                    ensure_write_file(self.get_file("memory.low"), &format!("{}", reservation))?;
                }
                if let Some(swap) = mem.swap() {
                    // TODO: memory swap is calculated differently in v2, but the spec is very v1 focussed.
                    // This code is almost certainly not right.
                    ensure_write_file(self.get_file("memory.swap.max"), &format!("{}", swap))?;
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

            if let Some(unified) = res.unified() {
                for (k, v) in unified {
                    ensure_write_file(self.get_file(k), v)?;
                }
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
        let stat = nix::sys::statfs::statfs(v1.join("cpu").as_path())
            .map_err(|e| std::io::Error::from(e))?;
        let is_v1 = stat.filesystem_type() == nix::sys::statfs::CGROUP_SUPER_MAGIC;
        if is_v1 {
            if v2.exists() {
                // Check if we are running in a hybrid cgroup setup
                let data = std::fs::read("/sys/fs/cgroup/unified/cgroup.controllers")
                    .map_err(|e| std::io::Error::from(e))?;

                let trimmed = std::str::from_utf8(&data)
                    .map_err(|e| {
                        Error::Others(format!(
                            "could not convert cgroup.controllers to string: {}",
                            e
                        ))
                    })?
                    .trim();

                if trimmed.len() > 0 {
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
        let stat = nix::sys::statfs::statfs(v1.as_path()).map_err(|e| std::io::Error::from(e))?;
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
        let p = PathBuf::from(base.clone()).join("cgroup.controllers");
        if std::fs::read(p).map_err(|e| std::io::Error::from(e))?.len() > 0 {
            return Ok(Box::new(CgroupV1 {
                base: base,
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
