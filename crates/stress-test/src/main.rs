mod mocks;
mod protos;
mod utils;

use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use nix::sys::prctl::set_child_subreaper;
use tokio::{sync::Semaphore, time::sleep};
use tokio::task::JoinSet;
use tokio::time::Duration;
use utils::{reap_children, TryFutureEx};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    /// Show the logs shim logs in stderr
    verbose: bool,

    /// Path to the shim binary
    shim: PathBuf,

    #[arg(short, long)]
    /// Create and start all tasks concurrently instead of one after the other
    parallel: bool,

    #[arg(short('n'), long, default_value("10"))]
    /// Number of tasks to run
    count: u32,

    #[arg(short, long, default_value("0"))]
    /// Runtime timeout in seconds, 0 = no timeout
    timeout: u64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    set_child_subreaper(true)?;
    let res = main_impl().await;
    let _ = reap_children().await?;
    res
}

async fn main_impl() -> Result<()> {
    env_logger::try_init()?;

    let Cli {
        shim,
        verbose,
        parallel,
        count,
        timeout,
    } = Cli::parse();
    let shim: String = shim.to_string_lossy().into_owned();

    let c8d = mocks::Containerd::new(verbose).await?;

    let shim = c8d.start_shim(shim).await?;
    let shim = Arc::new(shim);

    let permits = if !parallel { 1 } else { count as _ };
    let semaphore = Arc::new(Semaphore::new(permits));
    let successes = Arc::new(AtomicU32::new(0));
    let mut tracker: JoinSet<Result<()>> = JoinSet::new();

    for _ in 0..count {
        let shim = shim.clone();
        let semaphore = semaphore.clone();
        let successes = successes.clone();

        tracker.spawn(async move {
            let task = shim.task().await?;

            {
                // take a permit as this saction might have to run serially
                let _permit = semaphore.acquire().await?;
                task.create().await?;
                task.start().await?;
            }

            task.wait().await?;
            task.delete().await?;
            successes.fetch_add(1, SeqCst);

            Ok(())
        });
    }

    println!("Waiting for tasks to finish.");
    println!("Press Ctrl-C to terminate.");

    async {
        tracker.join_all().await;
        shim.shutdown().await?;
        c8d.shutdown().await
    }
    .with_timeout(Duration::from_secs(timeout))
    .or_ctrl_c()
    .await
    .with_context(|| anyhow!("{} tasks failed to run", count - successes.load(SeqCst)))?;

    Ok(())
}
