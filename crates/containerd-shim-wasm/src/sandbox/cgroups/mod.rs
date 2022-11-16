use super::{Error, Result};
use nix::sys::statfs;
pub use oci_spec::runtime::LinuxResources as Resources;
use proc_mounts::MountIter;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::prelude::Write;
use std::io::{BufRead, Read};
use std::ops::Not;
use std::os::raw::c_int as RawFD;
use std::path::PathBuf;

pub mod cgroupv1;
pub mod cgroupv2;

pub use cgroupv1::CgroupV1;
pub use cgroupv2::CgroupV2;

#[derive(Debug, PartialEq)]
pub enum Version {
    V1,
    V2,
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let v = match self {
            Version::V1 => "v1",
            Version::V2 => "v2",
        };

        write!(f, "{}", v)
    }
}

type FileMountIter = std::io::BufReader<fs::File>;

fn new_mount_iter() -> Result<MountIter<FileMountIter>> {
    Ok(MountIter::new()?)
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

impl TryFrom<&str> for Box<dyn Cgroup> {
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

impl<T: std::io::BufRead> TryFrom<CgroupOptions<T>> for Box<dyn Cgroup> {
    type Error = Error;

    fn try_from(opts: CgroupOptions<T>) -> Result<Self> {
        // Read mounts up front so we don't have to do it again (potentially twice) later.
        let f = fs::File::open("/proc/cgroups")?;
        let mounts = find_cgroup_mounts((opts.mounts)()?, &list_cgroup_controllers(f)?)?;

        if mounts.v1.is_empty().not() && mounts.v2.is_some() {
            if fs::read_to_string(mounts.v2.as_ref().unwrap().join("cgroup.controllers"))?
                .trim()
                .is_empty()
                .not()
            {
                return Err(Error::FailedPrecondition(
                    "cgroup v1 and v2 mounts found: hybrid cgroup mode is not supported"
                        .to_string(),
                ));
            }
        }

        // Here the caller passed in a root dir so we'll try to use that with v1/v2 directly
        if let Some(root) = &opts.root {
            let stat = statfs::statfs(root)?;
            if stat.filesystem_type() == statfs::CGROUP2_SUPER_MAGIC {
                if let Ok(cg) = CgroupV2::try_from(&opts) {
                    return Ok(Box::new(cg));
                }
            }

            // root path is not cgroup2 so it should be cgroup1
            if let Ok(cg) = CgroupV1::try_from(&opts) {
                return Ok(Box::new(cg));
            }
        }

        // Here we have already found the root dir for one or the other so we'll
        // use that and prevent having to iterate the mount table again.
        if mounts.v1.is_empty().not() {
            let opts = CgroupOptions {
                root: None,
                mounts: opts.mounts,
                name: opts.name,
                controllers: Some(mounts.v1),
            };
            let cg = CgroupV1::try_from(&opts)?;
            return Ok(Box::new(cg));
        }
        if mounts.v2.is_some() {
            let opts = CgroupOptions {
                root: mounts.v2,
                mounts: opts.mounts,
                name: opts.name,
                controllers: opts.controllers,
            };
            let cg = CgroupV2::try_from(&opts)?;
            return Ok(Box::new(cg));
        }

        Err(Error::FailedPrecondition(
            "cgroup mount not found".to_string(),
        ))
    }
}

fn safe_join(p1: PathBuf, p2: PathBuf) -> Result<PathBuf> {
    let mut p2 = p2;
    while p2.is_absolute() {
        p2 = match p2.strip_prefix("/") {
            Ok(p) => p.to_path_buf(),
            Err(_) => break,
        };
    }
    Ok(p1.join(p2))
}

pub fn new(name: String) -> Result<Box<dyn Cgroup>> {
    name.as_str().try_into()
}

pub struct CgroupOptions<T: std::io::BufRead> {
    pub mounts: fn() -> Result<MountIter<T>>,
    pub name: String,
    pub root: Option<PathBuf>,
    pub controllers: Option<HashMap<String, PathBuf>>,
}

fn ensure_write_file(path: std::path::PathBuf, content: &str) -> Result<()> {
    let parent = path.parent().unwrap();
    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|e| {
            Error::Others(format!(
                "could not create parent cgroup dir {}: {}",
                parent.to_str().unwrap(),
                e
            ))
        })?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|e| {
            Error::Others(format!(
                "could not open cgroup file {}: {}",
                path.to_str().unwrap(),
                e
            ))
        })?;
    file.write_all(content.as_bytes())
        .map_err(|e| Error::Others(format!("error writing cgroup values: {}", e)))?;
    Ok(())
}

struct CgroupMounts {
    v1: HashMap<String, PathBuf>,
    v2: Option<PathBuf>,
}

fn list_cgroup_controllers<R: Read>(r: R) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    for line in std::io::BufReader::new(r).lines() {
        let line = line?;
        let line = line.trim();
        if line.starts_with("#") {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let Some(kind) = parts.next() {
            set.insert(kind.to_string());
        }
    }
    Ok(set)
}

fn find_cgroup_mounts<T: BufRead>(
    iter: MountIter<T>,
    controllers: &HashSet<String>,
) -> Result<CgroupMounts> {
    let mut v1 = HashMap::new();
    let mut v2 = None;

    for mount in iter {
        let mount = mount?;
        match mount.fstype.as_str() {
            "cgroup" => {
                // Note: Multiple controllerse can be mounted to the same place
                // We need to look at all the mount options to determine which controllers are mounted at the given path
                for opt in mount.options.iter() {
                    if controllers.contains(opt) {
                        v1.insert(opt.to_string(), mount.dest.clone());
                    }
                }
            }
            "cgroup2" => {
                v2 = Some(mount.dest);
            }
            _ => {}
        }

        if v1.len() == controllers.len() && v2.is_some() {
            // We have everything we need, no need to keep itterating the mount table
            break;
        }
    }

    Ok(CgroupMounts { v1, v2 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_spec::runtime::Spec;
    use serde_json as json;
    use std::io::{Cursor, Write};

    use super::super::testutil::*;

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
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }
        let cg = new("containerd-wasm-shim-test_cgroup".to_string())?;
        let res = cgroup_test(Box::new(&*cg));
        if cg.version() == Version::V2 {
            match cg.open() {
                Ok(_) => {}
                Err(e) => {
                    cg.delete()?;
                    return Err(e);
                }
            }
        }
        cg.delete()?;
        res?;

        let cg = new("relative/nested/containerd-wasm-shim-test_cgroup".to_string())?;
        let res = cgroup_test(Box::new(&*cg));
        if cg.version() == Version::V2 {
            match cg.open() {
                Ok(_) => {}
                Err(e) => {
                    cg.delete()?;
                    return Err(e);
                }
            }
        }
        cg.delete()?;
        res?;

        let cg = new("/absolute/nested/containerd-wasm-shim-test_cgroup".to_string())?;
        let res = cgroup_test(Box::new(&*cg));
        if cg.version() == Version::V2 {
            match cg.open() {
                Ok(_) => {}
                Err(e) => {
                    cg.delete()?;
                    return Err(e);
                }
            }
        }
        cg.delete()?;
        res
    }

    fn new_test_mount_iter(data: &[u8]) -> MountIter<std::io::BufReader<std::io::Cursor<Vec<u8>>>> {
        let mut f = Cursor::new(Vec::new());
        f.write_all(STANDARD_MOUNTS).unwrap();
        f.write_all(data).unwrap();
        f.set_position(0);
        MountIter::new_from_reader(std::io::BufReader::new(f))
    }

    // Just some standard mounts to add to our fake mount table
    const STANDARD_MOUNTS: &[u8] = b"
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
udev /dev devtmpfs rw,nosuid,relatime,size=8173984k,nr_inodes=2043496,mode=755,inode64 0 0
devpts /dev/pts devpts rw,nosuid,noexec,relatime,gid=5,mode=620,ptmxmode=000 0 0
tmpfs /run tmpfs rw,nosuid,nodev,noexec,relatime,size=1638760k,mode=755,inode64 0 0
/dev/sda1 / ext4 rw,relatime,discard 0 0
securityfs /sys/kernel/security securityfs rw,nosuid,nodev,noexec,relatime 0 0
tmpfs /dev/shm tmpfs rw,nosuid,nodev,inode64 0 0
tmpfs /run/lock tmpfs rw,nosuid,nodev,noexec,relatime,size=5120k,inode64 0 0
";

    // cgroup v1 mounts to construct a fake mount table with
    const V1_ONLY: &[u8] = b"
tmpfs /sys/fs/cgroup tmpfs ro,nosuid,nodev,noexec,size=4096k,nr_inodes=1024,mode=755,inode64 0 0
cgroup /sys/fs/cgroup/cpu,cpuacct cgroup rw,nosuid,nodev,noexec,relatime,cpu,cpuacct 0 0
cgroup /sys/fs/cgroup/net_cls,net_prio cgroup rw,nosuid,nodev,noexec,relatime,net_cls,net_prio 0 0
cgroup /sys/fs/cgroup/pids cgroup rw,nosuid,nodev,noexec,relatime,pids 0 0
cgroup /sys/fs/cgroup/hugetlb cgroup rw,nosuid,nodev,noexec,relatime,hugetlb 0 0
cgroup /sys/fs/cgroup/cpuset cgroup rw,nosuid,nodev,noexec,relatime,cpuset 0 0
cgroup /sys/fs/cgroup/blkio cgroup rw,nosuid,nodev,noexec,relatime,blkio 0 0
cgroup /sys/fs/cgroup/rdma cgroup rw,nosuid,nodev,noexec,relatime,rdma 0 0
cgroup /sys/fs/cgroup/misc cgroup rw,nosuid,nodev,noexec,relatime,misc 0 0
cgroup /sys/fs/cgroup/perf_event cgroup rw,nosuid,nodev,noexec,relatime,perf_event 0 0
cgroup /sys/fs/cgroup/freezer cgroup rw,nosuid,nodev,noexec,relatime,freezer 0 0
cgroup /sys/fs/cgroup/memory cgroup rw,nosuid,nodev,noexec,relatime,memory 0 0
cgroup /sys/fs/cgroup/devices cgroup rw,nosuid,nodev,noexec,relatime,devices 0 0
";

    const V2_ONLY: &[u8] = b"
cgroup2 /sys/fs/cgroup cgroup2 rw,nosuid,nodev,noexec,relatime,nsdelegate 0 0
";

    const V2_UNIFIED: &[u8] = b"
cgroup2 /sys/fs/cgroup/unified cgroup2 rw,nosuid,nodev,noexec,relatime,nsdelegate 0 0
";

    const CGROUP_CONTROLLERS: &[u8] = b"
#subsys_name    hierarchy       num_cgroups     enabled
cpuset  6       3       1
cpu     2       89      1
cpuacct 2       89      1
blkio   7       80      1
memory  12      151     1
devices 13      80      1
freezer 11      4       1
net_cls 3       3       1
perf_event      10      3       1
net_prio        3       3       1
hugetlb 5       3       1
pids    4       94      1
rdma    8       3       1
misc    9       1       1
";

    #[test]
    fn test_find_cgroup_mounts() -> Result<()> {
        // Just V1

        let controllers = list_cgroup_controllers(Cursor::new(CGROUP_CONTROLLERS))?;
        let mounts = find_cgroup_mounts(new_test_mount_iter(V1_ONLY), &controllers)?;
        assert!(
            mounts.v1.is_empty().not() && mounts.v2.is_none(),
            "v1: {:?}, v2: {:?}, controllers: {:?}",
            mounts.v1,
            mounts.v2,
            controllers,
        );

        for ctrl in &controllers {
            assert!(mounts.v1.contains_key(ctrl));
        }

        // Just V2
        let mounts = find_cgroup_mounts(new_test_mount_iter(V2_ONLY), &controllers)?;
        assert!(mounts.v1.is_empty() && mounts.v2.is_some());
        assert!(mounts.v2.unwrap() == PathBuf::from("/sys/fs/cgroup"));

        // V1 with V2 unified
        let combined: Vec<u8> = V1_ONLY.iter().chain(V2_UNIFIED.iter()).copied().collect();
        let mounts = find_cgroup_mounts(new_test_mount_iter(&combined), &controllers)?;
        assert!(mounts.v1.is_empty().not() && mounts.v2.is_some());
        assert!(mounts.v1.len() == 14,); // 14 controllers even thoough there's only 12 mounts due to co-mounted controllers.
        assert!(mounts.v2.unwrap() == PathBuf::from("/sys/fs/cgroup/unified"));
        Ok(())
    }
}
