use super::error::Error;
use caps::{CapSet, Capability};
use clone3::Clone3;
use log::debug;
use nix::sys::signal::SIGCHLD;
use nix::sys::wait::{waitid, Id as WaitID, WaitPidFlag, WaitStatus};
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

fn has_cap_sys_admin() -> bool {
    let caps = caps::read(None, CapSet::Effective).unwrap();
    caps.contains(&Capability::CAP_SYS_ADMIN)
}

// perform_start can be used to setup a new thread to run your code in.
//
// This works like fork semantics:
// If this is the parent, the result will be Ok(Some(pid)), whre the pid is the pid of the new process.
// If this is the child, the result will be Ok(None).
//
// Code that runs in the child must not do things like access locks or other shared state.
//
// Optionally you can pass in a reference to a file descriptor which will be populated with the pidfd (see pidfd_open(2)).
pub unsafe fn perform_start(pidfd: Option<&mut PidFD>) -> Result<Context, Error> {
    let mut builder = Clone3::default();

    if pidfd.is_some() {
        let fd = pidfd.unwrap();
        builder.flag_pidfd(fd);
    };

    builder.exit_signal(SIGCHLD as u64).flag_ptrace();
    if has_cap_sys_admin() {
        builder
            .flag_newpid()
            .flag_newuts()
            .flag_newipc()
            .flag_newcgroup()
            .flag_newns();
    } else {
        debug!("no CAP_SYS_ADMIN, not creating new namespaces");
    }

    match { builder.call() } {
        Ok(tid) => match tid {
            0 => Ok(Context::Child),
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
        _ => Err(Error::Others(format!("unexpected wait status: {:?}", info))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perform_start() -> Result<(), Error> {
        let test_exit_code = 42;
        unsafe {
            let mut pidfd = RawFD::from(-1);
            match perform_start(Some(&mut pidfd)) {
                Ok(Context::Parent(tid)) => {
                    assert!(tid > 0);
                    let status = wait_for_pidfd(pidfd)?;
                    if status.status != test_exit_code {
                        return Err(Error::Others(format!(
                            "unexpected exit status: {:?}",
                            status
                        )));
                    }
                    Ok(())
                }
                Ok(Context::Child) => std::process::exit(test_exit_code as i32),
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }
}
