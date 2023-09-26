use std::fs::File;

use anyhow::Result;
use containerd_shim::error::Error as ShimError;
use containerd_shim::{self as shim};
use nix::sched::{setns, unshare, CloneFlags};
use oci_spec::runtime;

pub fn setup_namespaces(spec: &runtime::Spec) -> Result<()> {
    let namespaces = spec
        .linux()
        .as_ref()
        .unwrap()
        .namespaces()
        .as_ref()
        .unwrap();
    for ns in namespaces {
        if ns.typ() == runtime::LinuxNamespaceType::Network {
            if let Some(p) = ns.path() {
                let f = File::open(p).map_err(|err| {
                    ShimError::Other(format!(
                        "could not open network namespace {}: {}",
                        p.display(),
                        err
                    ))
                })?;
                setns(f, CloneFlags::CLONE_NEWNET).map_err(|err| {
                    ShimError::Other(format!("could not set network namespace: {0}", err))
                })?;
            } else {
                unshare(CloneFlags::CLONE_NEWNET).map_err(|err| {
                    ShimError::Other(format!("could not unshare network namespace: {0}", err))
                })?;
            }
        }
    }

    // Keep all mounts changes (such as for the rootfs) private to the shim
    // This way mounts will automatically be cleaned up when the shim exits.
    unshare(CloneFlags::CLONE_NEWNS)
        .map_err(|err| shim::Error::Other(format!("failed to unshare mount namespace: {}", err)))?;
    Ok(())
}
