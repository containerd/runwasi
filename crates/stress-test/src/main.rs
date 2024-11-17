mod mocks;
mod protos;
mod utils;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use nix::sys::prctl::set_child_subreaper;
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinSet;
use tokio::time::Duration;
use utils::{reap_children, TryFutureEx};

#[derive(ValueEnum, Clone, Copy)]
enum Step {
    Create,
    Start,
    Wait,
    Delete,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    /// Show the shim logs in stderr
    verbose: bool,

    #[arg(short('O'), long)]
    /// Show the container output in stdout
    container_output: bool,

    /// Path to the shim binary
    shim: PathBuf,

    #[arg(short, long, default_value("1"))]
    /// Number of tasks to create and start concurrently [0 = no limit]
    parallel: usize,

    #[arg(short('S'), long, default_value("start"))]
    /// Up to what steps to run in series
    serial_steps: Step,

    #[arg(short('n'), long, default_value("10"))]
    /// Number of tasks to run
    count: usize,

    #[clap(short, long, value_parser = humantime::parse_duration, default_value = "2s")]
    /// Runtime timeout [0 = no timeout]
    timeout: Duration,
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
        container_output,
        parallel,
        serial_steps,
        count,
        timeout,
    } = Cli::parse();
    let shim: String = shim.to_string_lossy().into_owned();

    let c8d = mocks::Containerd::new(verbose).await?;

    let shim = c8d.start_shim(shim).await?;
    let shim = Arc::new(shim);

    let permits = if parallel == 0 {
        count as _
    } else {
        parallel as _
    };
    let semaphore = Arc::new(Semaphore::new(permits));
    let mut tracker: JoinSet<Result<()>> = JoinSet::new();

    let serial_steps = match serial_steps {
        Step::Create => "c",
        Step::Start => "cs",
        Step::Wait => "csw",
        Step::Delete => "cswd",
    };

    for _ in 0..count {
        let shim = shim.clone();
        let semaphore = semaphore.clone();

        tracker.spawn(async move {
            let task = shim.task().await?;

            {
                // take a permit as this saction might have to run serially
                let _permit = semaphore.acquire().await?;
                if serial_steps.contains('c') {
                    task.create(container_output).await?;
                }
                if serial_steps.contains('s') {
                    task.start().await?;
                }
                if serial_steps.contains('w') {
                    task.wait().await?;
                }
                if serial_steps.contains('d') {
                    task.delete().await?;
                }
            }

            if !serial_steps.contains('c') {
                task.create(container_output).await?;
            }
            if !serial_steps.contains('s') {
                task.start().await?;
            }
            if !serial_steps.contains('w') {
                task.wait().await?;
            }
            if !serial_steps.contains('d') {
                task.delete().await?;
            }

            Ok(())
        });
    }

    println!("Waiting for tasks to finish.");
    println!("Press Ctrl-C to terminate.");

    let mut success = 0;
    let mut failed = 0;
    let ping = Arc::new(Notify::new());
    let _ = async {
        let ping = ping.clone();
        let count = count as usize;
        while let Some(res) = tracker.join_next().await {
            ping.notify_waiters();
            match res {
                Ok(Ok(())) => {
                    success += 1;
                    println!(" > {} .. [OK]", count - tracker.len());
                }
                Ok(Err(err)) => {
                    failed += 1;
                    println!(" > {} .. {err}", count - tracker.len());
                }
                Err(err) => {
                    println!(" > {} .. {err}", count - tracker.len());
                }
            }
        }
        let _ = shim.shutdown().await;
        c8d.shutdown().await
    }
    .with_watchdog(timeout, ping.clone())
    .or_ctrl_c()
    .await;

    println!();
    println!("{success} tasks succeeded");
    println!("{failed} tasks failed");
    println!("{} tasks hanged", count - success - failed);

    Ok(())
}
