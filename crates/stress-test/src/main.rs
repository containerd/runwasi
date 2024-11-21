mod mocks;
mod protos;
mod utils;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use humantime::{format_duration, parse_duration};
use nix::sys::prctl::set_child_subreaper;
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinSet;
use tokio::time::{Duration, Instant};
use utils::{reap_children, DropIf as _, TryFutureEx as _};

#[derive(ValueEnum, Clone, Copy, PartialEq)]
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

    #[clap(short, long, value_parser = parse_duration, default_value = "2s")]
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

    let mut tasks = vec![];

    // First create the tasks bundles, as this is not
    // work done by the shim itself
    for _ in 0..count {
        let task = shim.task().await?;
        tasks.push(task);
    }

    let permits = if parallel == 0 {
        count as _
    } else {
        parallel as _
    };
    let semaphore = Arc::new(Semaphore::new(permits));
    let mut tracker: JoinSet<Result<()>> = JoinSet::new();

    let start = Instant::now();
    for task in tasks {
        let semaphore = semaphore.clone();

        tracker.spawn(async move {
            // take a permit as this section might have to run serially
            let mut permit = Some(semaphore.acquire().await?);

            task.create(container_output).await?;
            permit.drop_if(serial_steps == Step::Create);

            task.start().await?;
            permit.drop_if(serial_steps == Step::Start);

            task.wait().await?;
            permit.drop_if(serial_steps == Step::Wait);

            task.delete().await?;
            permit.drop_if(serial_steps == Step::Delete);

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
        Ok(())
    }
    .with_watchdog(timeout, ping.clone())
    .or_ctrl_c()
    .await;

    let elapsed = start.elapsed();

    let color = if success == count { 32 } else { 31 };

    println!();
    println!(
        "\x1b[{color}m{success} tasks succeeded, {failed} tasks failed, {} tasks didn't finish\x1b[0m",
        count - success - failed
    );
    println!("elapsed time: {}", format_duration(elapsed));

    Ok(())
}
