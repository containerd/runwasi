//! This module includes a test for sending signals to containers when
//! the shim is managing two or more containers.
//!
//! See https://github.com/containerd/runwasi/issues/755 for context.
//!
//! This test is currently broken for the reasons explained in #755.
//! Running the test will result in a failure:
//! ```
//! cargo test -p containerd-shim-wasm -- test::signals::test_handling_signals --exact --show-output --nocapture --ignored
//! ```
//!
//! This is because the current implementation breaks `tokio::signal`.
//! You can verify this by using `libc::signal` instead, and the test will succeed
//! ```
//! USE_LIBC=1 cargo test -p containerd-shim-wasm -- test::signals::test_handling_signals --exact --show-output --nocapture --ignored
//! ```
//!
//! Once #755 is fixed we can remove the libc based implementation and
//! remove the ignore attribute from the test.

use std::future::pending;
use std::sync::mpsc::channel;
use std::sync::{Arc, LazyLock};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{bail, Result};
use containerd_shim_wasm_test_modules::HELLO_WORLD;
use tokio::sync::Notify;

use crate::container::{Engine, Instance, RuntimeContext};
use crate::sandbox::Stdio;
use crate::testing::WasiTest;

#[derive(Clone, Default)]
pub struct SomeEngine;

async fn ctrl_c(use_libc: bool) {
    static CANCELLATION: LazyLock<Notify> = LazyLock::new(|| Notify::new());

    fn on_ctr_c(_: libc::c_int) {
        CANCELLATION.notify_waiters();
    }

    if use_libc {
        unsafe { libc::signal(libc::SIGINT, on_ctr_c as _) };
        CANCELLATION.notified().await;
    } else {
        let _ = tokio::signal::ctrl_c().await;
    }
}

impl Engine for SomeEngine {
    fn name() -> &'static str {
        "some-engine"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        stdio.redirect()?;
        let name = ctx.entrypoint().func;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async move {
                use tokio::time::sleep;
                let use_libc = std::env::var("USE_LIBC").unwrap_or_default();
                let use_libc = !use_libc.is_empty() && use_libc != "0";
                let signal = async {
                    println!("{name}> waiting for signal!");
                    ctrl_c(use_libc).await;
                    println!("{name}> received signal, bye!");
                };
                let task = async {
                    sleep(Duration::from_millis(10)).await;
                    eprintln!("{name}> ready");
                    pending().await
                };
                tokio::select! {
                    _ = signal => {}
                    _ = task => {}
                };
                Ok(0)
            })
    }
}

type SomeInstance = Instance<SomeEngine>;

struct KillGuard(Arc<WasiTest<SomeInstance>>);
impl Drop for KillGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

#[test]
#[ignore = "this currently fails due to tokio's global state"]
fn test_handling_signals() -> Result<()> {
    // use a thread scope to ensure we join all threads at the end
    std::thread::scope(|s| -> Result<()> {
        let mut containers = vec![];

        for i in 0..20 {
            let container = WasiTest::<SomeInstance>::builder()?
                .with_name(format!("test-{i}"))
                .with_start_fn(format!("test-{i}"))
                .with_stdout("/proc/self/fd/1")?
                .with_wasm(HELLO_WORLD)?
                .build()?;
            containers.push(Arc::new(container));
        }

        let guard: Vec<_> = containers.iter().cloned().map(KillGuard).collect();

        for container in containers.iter() {
            container.start()?;
        }

        let (tx, rx) = channel();

        for (i, container) in containers.iter().cloned().enumerate() {
            let tx = tx.clone();
            s.spawn(move || -> anyhow::Result<()> {
                println!("shim> waiting for container {i}");
                let (code, ..) = container.wait(Duration::from_secs(10000))?;
                println!("shim> container test-{i} exited with code {code}");
                tx.send(i)?;
                Ok(())
            });
        }

        'outer: for (i, container) in containers.iter().enumerate() {
            for _ in 0..100 {
                let stderr = container.read_stderr()?.unwrap_or_default();
                if stderr.contains("ready") {
                    continue 'outer;
                }
                sleep(Duration::from_millis(1));
            }
            bail!("timeout waiting for container test-{i}");
        }

        println!("shim> all containers ready");

        for (i, container) in containers.iter().enumerate() {
            println!("shim> sending ctrl-c to container test-{i}");
            let _ = container.ctrl_c()?;
            let id = rx.recv_timeout(Duration::from_secs(5))?;
            println!("shim> received exit from container test-{id} (expected test-{i})");
            assert_eq!(id, i);
        }

        drop(guard);

        Ok(())
    })
}
