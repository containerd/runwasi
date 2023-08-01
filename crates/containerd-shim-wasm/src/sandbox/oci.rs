//! Generic helpers for working with OCI specs that can be consumed by any runtime.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::error::Result;
use anyhow::Context;
use nix::{sys::signal, unistd::Pid};
pub use oci_spec::runtime::Spec;
use serde_json as json;
use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::os::unix::process::CommandExt;
use std::process;

pub fn load(path: &str) -> Result<Spec> {
    let spec = Spec::load(path)?;
    Ok(spec)
}

pub fn get_root(spec: &Spec) -> &PathBuf {
    let root = spec.root().as_ref().unwrap();
    root.path()
}

pub fn get_args(spec: &Spec) -> &[String] {
    let p = match spec.process() {
        None => return &[],
        Some(p) => p,
    };

    match p.args() {
        None => &[],
        Some(args) => args.as_slice(),
    }
}

pub fn spec_from_file<P: AsRef<Path>>(path: P) -> Result<Spec> {
    let file = File::open(path)?;
    let cfg: Spec = json::from_reader(file)?;
    Ok(cfg)
}

fn parse_env(envs: &[String]) -> HashMap<String, String> {
    // make NAME=VALUE to HashMap<NAME, VALUE>.
    envs.iter()
        .filter_map(|e| {
            let mut split = e.split('=');

            split.next().map(|key| {
                let value = split.collect::<Vec<&str>>().join("=");
                (key.into(), value)
            })
        })
        .collect()
}

pub fn setup_prestart_hooks(hooks: &Option<oci_spec::runtime::Hooks>) -> Result<()> {
    if let Some(hooks) = hooks {
        let prestart_hooks = hooks.prestart().as_ref().unwrap();

        for hook in prestart_hooks {
            let mut hook_command = process::Command::new(hook.path());
            // Based on OCI spec, the first argument of the args vector is the
            // arg0, which can be different from the path.  For example, path
            // may be "/usr/bin/true" and arg0 is set to "true". However, rust
            // command differenciates arg0 from args, where rust command arg
            // doesn't include arg0. So we have to make the split arg0 from the
            // rest of args.
            if let Some((arg0, args)) = hook.args().as_ref().and_then(|a| a.split_first()) {
                log::debug!("run_hooks arg0: {:?}, args: {:?}", arg0, args);
                hook_command.arg0(arg0).args(args)
            } else {
                hook_command.arg0(&hook.path().display().to_string())
            };

            let envs: HashMap<String, String> = if let Some(env) = hook.env() {
                parse_env(env)
            } else {
                HashMap::new()
            };
            log::debug!("run_hooks envs: {:?}", envs);

            let mut hook_process = hook_command
                .env_clear()
                .envs(envs)
                .stdin(process::Stdio::piped())
                .spawn()
                .with_context(|| "Failed to execute hook")?;
            let hook_process_pid = Pid::from_raw(hook_process.id() as i32);

            if let Some(stdin) = &mut hook_process.stdin {
                // We want to ignore BrokenPipe here. A BrokenPipe indicates
                // either the hook is crashed/errored or it ran successfully.
                // Either way, this is an indication that the hook command
                // finished execution.  If the hook command was successful,
                // which we will check later in this function, we should not
                // fail this step here. We still want to check for all the other
                // error, in the case that the hook command is waiting for us to
                // write to stdin.
                let state = format!("{{ \"pid\": {} }}", std::process::id());
                if let Err(e) = stdin.write_all(state.as_bytes()) {
                    if e.kind() != ErrorKind::BrokenPipe {
                        // Not a broken pipe. The hook command may be waiting
                        // for us.
                        let _ = signal::kill(hook_process_pid, signal::Signal::SIGKILL);
                    }
                }
            }
            hook_process.wait()?;
        }
    }
    Ok(())
}
