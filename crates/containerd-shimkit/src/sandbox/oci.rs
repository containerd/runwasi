//! Generic helpers for working with OCI specs that can be consumed by any runtime.

use std::collections::HashMap;
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process;

use anyhow::Context;

use super::error::Result;

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

pub(crate) fn setup_prestart_hooks(hooks: &Option<oci_spec::runtime::Hooks>) -> Result<()> {
    if let Some(hooks) = hooks {
        let Some(prestart_hooks) = hooks.prestart().as_ref() else {
            return Ok(());
        };

        for hook in prestart_hooks {
            let mut hook_command = process::Command::new(hook.path());
            // Based on OCI spec, the first argument of the args vector is the
            // arg0, which can be different from the path.  For example, path
            // may be "/usr/bin/true" and arg0 is set to "true". However, rust
            // command differentiates arg0 from args, where rust command arg
            // doesn't include arg0. So we have to make the split arg0 from the
            // rest of args.
            if let Some((arg0, args)) = hook.args().as_ref().and_then(|a| a.split_first()) {
                log::debug!("run_hooks arg0: {arg0:?}, args: {args:?}");

                #[cfg(unix)]
                {
                    hook_command.arg0(arg0).args(args);
                }

                #[cfg(windows)]
                {
                    if !&hook.path().ends_with(arg0) {
                        return Err(crate::sandbox::Error::InvalidArgument("Running with arg0 as different name than executable is not supported on Windows due to rust std library process implementation.".to_string()));
                    }

                    hook_command.args(args);
                }
            } else {
                #[cfg(unix)]
                hook_command.arg0(hook.path());
            };

            let envs: HashMap<String, String> = if let Some(env) = hook.env() {
                parse_env(env)
            } else {
                HashMap::new()
            };
            log::debug!("run_hooks envs: {envs:?}");

            let mut hook_process = hook_command
                .env_clear()
                .envs(envs)
                .stdin(process::Stdio::piped())
                .spawn()
                .with_context(|| "Failed to execute hook")?;

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
                if let Err(e) = stdin.write_all(state.as_bytes())
                    && e.kind() != ErrorKind::BrokenPipe
                {
                    // Not a broken pipe. The hook command may be waiting
                    // for us.
                    let _ = hook_process.kill();
                }
            }
            hook_process.wait()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use oci_spec::runtime::{HookBuilder, HooksBuilder};
    use tempfile::tempdir;

    use super::setup_prestart_hooks;

    #[test]
    fn setup_prestart_hooks_ignores_missing_prestart_list() {
        let hooks = HooksBuilder::default()
            .create_runtime(vec![
                HookBuilder::default().path("/bin/true").build().unwrap(),
            ])
            .build()
            .unwrap();

        setup_prestart_hooks(&Some(hooks)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn setup_prestart_hooks_still_runs_prestart_hooks() {
        let tempdir = tempdir().unwrap();
        let marker = tempdir.path().join("prestart-ran");
        let command = format!("touch {}", marker.display());
        let hooks = HooksBuilder::default()
            .prestart(vec![
                HookBuilder::default()
                    .path("/bin/sh")
                    .args(vec!["sh".to_string(), "-c".to_string(), command])
                    .build()
                    .unwrap(),
            ])
            .build()
            .unwrap();

        setup_prestart_hooks(&Some(hooks)).unwrap();

        assert!(fs::metadata(marker).is_ok());
    }
}
