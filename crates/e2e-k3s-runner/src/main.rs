use std::fs::{set_permissions, OpenOptions, Permissions};
use std::io::IsTerminal;
use std::os::unix::prelude::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Result};
use clap::Parser;
use itertools::Itertools;
use path_clean::PathClean;

/// Run e2e k3s tests
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// OCI image tar to import before running.
    #[arg(short, long, default_value = "dist/img.tar")]
    image: PathBuf,

    /// Path to the kubernetes deployment configuration yaml file.
    #[arg(short, long, default_value = "test/k8s/deploy.yaml")]
    deploy: PathBuf,

    /// Name of the deployment to test.
    #[arg(short = 'D', long, default_value = "wasi-demo")]
    deployment: String,

    /// Path to a directory where to store logs.
    #[arg(short, long)]
    logs: Option<PathBuf>,

    /// Path to shim executable to use in the test.
    #[arg(value_name = "SHIM")]
    shim: String,
}

macro_rules! path_buf {
    ($($e:expr),+) => { PathBuf::from(format!($($e),*)) };
}

fn is_executable(path: impl AsRef<Path>) -> bool {
    let Ok(metadata) = path.as_ref().metadata() else {
        return false;
    };
    metadata.is_file() && metadata.mode() & 0o111 != 0
}

fn find_shim(shim: &str) -> Result<PathBuf> {
    if shim.contains(std::path::MAIN_SEPARATOR) {
        ensure!(
            is_executable(shim),
            "Shim should be a path to an executable: {shim:?}"
        );
        return Ok(PathBuf::from(shim));
    }

    use std::env::consts::ARCH;
    let dirs = [
        path_buf!("target/build/{ARCH}-unknown-linux-musl/{ARCH}-unknown-linux-musl/debug"),
        path_buf!("target/build/{ARCH}-unknown-linux-musl/{ARCH}-unknown-linux-musl/release"),
        path_buf!("dist/bin"),
    ];
    let exes = [
        path_buf!("{shim}"),
        path_buf!("containerd-shim-{shim}-v1"),
        path_buf!("containerd-shim-{shim}-v2"),
    ];

    let Some(shim) = dirs
        .into_iter()
        .cartesian_product(exes)
        .map(|(dir, exe)| dir.join(exe))
        .find(|p| is_executable(p))
    else {
        bail!("Shim not found: {shim:?}");
    };

    Ok(shim)
}

fn mount_rw(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<String> {
    let dst = dst.as_ref().clean();
    let dst = dst.to_string_lossy();
    if !src.as_ref().as_os_str().is_empty() {
        let src = src.as_ref().canonicalize()?;
        let src = src.to_string_lossy();
        Ok(format!("-v{src}:{dst}"))
    } else {
        Ok(format!("-v{dst}"))
    }
}

fn mount_ro(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<String> {
    Ok(mount_rw(src, dst)? + ":ro")
}

fn touch(path: impl AsRef<Path>) -> Result<()> {
    OpenOptions::new().create(true).write(true).open(path)?;
    Ok(())
}

fn is_cgroups_v2() -> bool {
    Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
}

fn main() -> Result<()> {
    let Args {
        image,
        deploy,
        logs,
        shim,
        deployment,
    } = Args::parse();

    ensure!(image.is_file(), "Image file not found: {image:?}");
    ensure!(deploy.is_file(), "Deployment file not found: {deploy:?}");

    let shim = find_shim(&shim)?;

    let tempdir = tempfile::tempdir()?;

    let logs = logs.unwrap_or_else(|| tempdir.path().join("logs"));
    let c8d_logs = logs.join("containerd.log");
    let _ = std::fs::create_dir_all(&logs);
    ensure!(logs.is_dir(), "Logs directory not found: {logs:?}");
    let _ = touch(&c8d_logs);
    ensure!(c8d_logs.is_file(), "Logs for c8d not found: {c8d_logs:?}");

    let Some(shim_name) = shim.file_name().map(|f| f.to_string_lossy().to_string()) else {
        bail!("Unrecognized shim name: {shim:?}");
    };
    let shim_dst = path_buf!("/shim/{shim_name}");

    let tty_arg = if std::io::stdout().is_terminal() {
        "-it"
    } else {
        "-i"
    };

    let config = tempdir.path().join("config.toml.tmpl");
    std::fs::write(
        &config,
        format!(include_str!("config.toml.tmpl"), shim_name = shim_name),
    )?;

    let entry = tempdir.path().join("entry.sh");
    std::fs::write(&entry, include_str!("entry.sh"))?;
    set_permissions(&entry, Permissions::from_mode(0o755))?;

    let dind = tempdir.path().join("dind");
    std::fs::write(&dind, include_str!("dind"))?;
    set_permissions(&dind, Permissions::from_mode(0o755))?;

    let cgroups_dst = if is_cgroups_v2() {
        "/sys/fs/cgroup/host"
    } else {
        "/sys/fs/cgroup"
    };

    println!(">> image:       {image:?}");
    println!(">> deploy:      {deploy:?}");
    println!(">> deployment:  {deployment:?}");
    println!(">> shim:        {shim:?}");
    println!(">> logs:        {logs:?}");
    println!();
    println!(">> Running e2e test");

    #[rustfmt::skip]
    let status = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("--entrypoint=/dind")
        .arg("--privileged")
        .arg(tty_arg)
        .arg(mount_rw("/sys/fs/cgroup", cgroups_dst)?)
        .arg(mount_ro(dind, "/dind")?)
        .arg(mount_ro(entry, "/entry.sh")?)
        .arg(mount_ro(config, "/var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl")?)
        .arg(mount_ro(shim, shim_dst)?)
        .arg(mount_ro(image, "/image/img.tar")?)
        .arg(mount_ro(deploy, "/deploy/deploy.yaml")?)
        .arg(mount_rw(logs, "/var/log")?)
        .arg(mount_rw(c8d_logs, "/var/lib/rancher/k3s/agent/containerd/containerd.log")?)
        .arg("rancher/k3s:v1.27.6-k3s1")
        .arg("/entry.sh")
        .arg(shim_name)
        .arg(deployment)
        .status()?;

    ensure!(status.success(), "{status}");

    Ok(())
}
