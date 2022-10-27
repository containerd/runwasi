use super::cgroups::{Cgroup, Version as CgroupVersion};
use super::error::Error;
use caps::{CapSet, Capability};
use clone3::Clone3;
use log::debug;
use log::info;
use nix::sys::signal::SIGCHLD;
use nix::sys::wait::{waitid, Id as WaitID, WaitPidFlag, WaitStatus};
use nix::unistd::close;
use std::os::raw::c_int as RawFD;

pub type PidFD = RawFD;

#[derive(Debug)]
pub struct ExitStatus {
    pub pid: u32,
    pub status: u32,
}

pub enum Context {
    Parent(u32),
    Child,
}

pub fn has_cap_sys_admin() -> bool {
    let caps = caps::read(None, CapSet::Effective).unwrap();
    caps.contains(&Capability::CAP_SYS_ADMIN)
}

// This is is a wrapper around clone3 which allows us to pass a pidfd
// This works otherwise just like normal forking semantics:
// If this is the parent, the result will be Ok(Context::Parent(pid)), where the pid is the pid of the new process.
// If this is the child, the result will be Ok(Context::Child).
//
// Code that runs in the child must not do things like access locks or other shared state.
//
// This will also (currently) spawn the new process in a new set of namespaces (except for network namespaces).
// This may change as the utility of such behavior is evaluated.
//
// Optionally you can pass in a reference to a file descriptor which will be populated with the pidfd (see pidfd_open(2)).
pub unsafe fn fork(
    cgroup: Option<&Box<dyn Cgroup>>,
    pidfd: Option<&mut PidFD>,
) -> Result<Context, Error> {
    let mut builder = Clone3::default();

    if pidfd.is_some() {
        let fd = pidfd.unwrap();
        builder.flag_pidfd(fd);
    };

    builder.exit_signal(SIGCHLD as u64).flag_ptrace();

    let is_root = has_cap_sys_admin();

    let mut fd: RawFD = -1; // Keep the fd alive until we return
    if is_root {
        if let Some(cgroup) = &cgroup {
            if cgroup.version() == CgroupVersion::V2 {
                fd = cgroup.open()?;
                builder.flag_into_cgroup(&fd);
            }
        }

        builder
            .flag_newpid()
            .flag_newuts()
            .flag_newipc()
            .flag_newcgroup()
            .flag_newns();
    } else {
        debug!("no CAP_SYS_ADMIN, not creating new namespaces");
    }

    let res = { builder.call() };
    if fd > -1 {
        match close(fd) {
            Ok(_) => {}
            Err(e) => {
                info!("failed to close cgroup fd: {}", e);
            }
        }
    }
    match res {
        Ok(tid) => match tid {
            0 => {
                if is_root {
                    if let Some(cgroup) = cgroup {
                        // With v2 we use clone_into_cgroup, so we only want to handle this for v1
                        if cgroup.version() == CgroupVersion::V1 {
                            cgroup.add_task(std::process::id()).map_err(|e| {
                                Error::Others(format!("error adding pid to cgroup: {}", e))
                            })?;
                        }
                    }
                }
                Ok(Context::Child)
            }
            _ => Ok(Context::Parent(tid as u32)),
        },
        Err(e) => Err(std::io::Error::from(e).into()),
    }
}

// wait_for_child can be used in conjunction with the pidfd passed into perform_start
// to wait for the child to exit.
pub fn wait_for_pidfd(pidfd: PidFD) -> Result<ExitStatus, Error> {
    let info =
        waitid(WaitID::PIDFd(pidfd), WaitPidFlag::WEXITED).map_err(|e| std::io::Error::from(e))?;

    match info {
        WaitStatus::Exited(pid, status) => Ok(ExitStatus {
            pid: pid.as_raw() as u32,
            status: status as u32,
        }),
        WaitStatus::Signaled(pid, sig, dumped) => {
            info!("child {} killed by signal {}, dumped: {}", pid, sig, dumped);
            Ok(ExitStatus {
                pid: pid.as_raw() as u32,
                status: 128 + sig as u32,
            })
        }
        _ => Err(Error::Others(format!("unexpected wait status: {:?}", info))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::cgroups;
    use super::*;

    #[test]
    fn test_fork() -> Result<(), Error> {
        let test_exit_code = 42;
        let cg = cgroups::new("test_perform_start".to_string())?;

        let mut pidfd = RawFD::from(-1);

        unsafe {
            let ret = fork(Some(&cg), Some(&mut pidfd));
            match ret {
                Ok(Context::Parent(tid)) => {
                    // Wait for the child to exit before trying to delete the cgroup
                    let status = wait_for_pidfd(pidfd)
                        .map_err(|e| Error::Others(format!("error waiting for pidfd: {}", e)));

                    if has_cap_sys_admin() {
                        cg.delete()
                            .map_err(|e| Error::Others(format!("error deleting cgroup: {}", e)))?;
                    }
                    assert!(tid > 0);
                    let status = status?;
                    if status.status != test_exit_code {
                        return Err(Error::Others(format!(
                            "unexpected exit status: {:?}",
                            status
                        )));
                    }
                    return Ok(());
                }
                Ok(Context::Child) => std::process::exit(test_exit_code as i32),
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }
}
