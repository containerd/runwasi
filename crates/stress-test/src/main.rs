mod containerd;
mod mocks;
mod protos;
mod traits;
mod utils;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, bail};
use clap::{Parser, ValueEnum};
use futures::future::FusedFuture as _;
use futures::stream::FuturesUnordered;
use futures::{FutureExt as _, StreamExt as _};
use humantime::{format_duration, parse_duration};
use nix::sys::prctl::set_child_subreaper;
use serde::Serialize;
use tokio::signal::ctrl_c;
use tokio::sync::{Barrier, OnceCell, Semaphore};
use tokio::time::Duration;
use traits::{Containerd, Shim as _, Task as _};
use utils::{reap_children, watchdog};

#[derive(ValueEnum, Clone, Copy, PartialEq)]
enum Step {
    Create,
    Start,
    Wait,
    Delete,
}

#[derive(Serialize)]
struct BenchmarkResult {
    name: String,
    unit: String,
    value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<String>,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    /// Use containerd to manage the shim
    containerd: bool,

    #[arg(short, long)]
    /// Show the shim logs in stderr
    verbose: bool,

    #[arg(short, long, default_value("1"))]
    /// Number of tasks to create and start concurrently [0 = no limit]
    parallel: usize,

    #[arg(short('n'), long, default_value("10"))]
    /// Number of tasks to run
    count: usize,

    #[clap(short, long, value_parser = parse_duration, default_value = "2s")]
    /// Runtime timeout [0 = no timeout]
    timeout: Duration,

    #[clap(
        short,
        long,
        default_value = "ghcr.io/containerd/runwasi/wasi-demo-app:latest"
    )]
    /// Image to use for the test
    image: String,

    #[arg(long)]
    /// Output the benchmark results to a JSON file
    json_output: Option<PathBuf>,

    /// Path to the shim binary
    shim: PathBuf,

    #[clap(default_values = ["echo", "hello"])]
    /// Arguments to pass to the image
    args: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    set_child_subreaper(true)?;
    let res1 = main_impl().await;
    let res2 = reap_children().await;
    res1.and(res2)
}

async fn main_impl() -> Result<()> {
    env_logger::try_init()?;

    let cli = Cli::parse();

    if cli.containerd {
        let containerd = containerd::Containerd::new().await?;
        run_stress_test(cli, containerd).await
    } else {
        let containerd = mocks::Containerd::new(cli.verbose).await?;
        run_stress_test(cli, containerd).await
    }
}

fn get_runtime(path: &Path) -> Option<&str> {
    let runtime = path
        .file_name()?
        .to_str()?
        .strip_prefix("containerd-shim-")?
        .split_once('-')?
        .0;
    Some(runtime)
}

async fn run_stress_test(cli: Cli, c8d: impl Containerd) -> Result<()> {
    let Cli {
        containerd,
        shim: shim_path,
        parallel,
        count,
        timeout,
        image,
        json_output,
        args,
        ..
    } = cli;

    println!("\x1b[1mUsing image {image:?} with arguments {args:?}\x1b[0m");

    let shim = c8d.start_shim(shim_path.clone()).await?;
    let shim = Arc::new(shim);

    // create a "pause" container to keep the shim running
    let pause = shim.task(&image, &args).await?;
    pause.create().await?;

    let permits = if parallel == 0 { count } else { parallel };
    let semaphore = Arc::new(Semaphore::new(permits));
    let barrier = Arc::new(Barrier::new(count + 1));
    let start = Arc::new(OnceCell::new());
    let mut tracker = FuturesUnordered::new();
    let setup_start = Instant::now();

    for _ in 0..count {
        let shim = shim.clone();
        let image = image.clone();
        let args = args.clone();
        let semaphore = semaphore.clone();
        let barrier = barrier.clone();
        let start = start.clone();
        tracker.push(async move {
            // create the tasks bundles before starting measuring the benchmark
            // this is not work done by the shim itself
            let task = shim.task(image, args).await?;

            // wait for all tasks to be set up
            barrier.wait().await;

            // Wait for a concurrentcy slot
            let permit = semaphore.acquire_owned().await?;
            let _ = start.set(Instant::now());

            task.create().await?;
            task.start().await?;

            // release the concurrency slot
            drop(permit);

            task.wait().await?;
            task.delete().await?;

            Ok(())
        });
    }

    let setup_done = barrier.wait().fuse();
    let mut setup_done = pin!(setup_done);

    eprintln!("> Setting up tasks.");
    eprintln!("  Press Ctrl-C to terminate.\x1b[A");

    let mut incomplete = count;
    let mut success = 0;
    let mut failed = 0;
    let mut clear_line = false;

    loop {
        tokio::select! {
            _ = &mut setup_done => {
                let elapsed = setup_start.elapsed();
                let elapsed = format_duration(elapsed);
                eprint!("\x1b[2K");
                eprintln!("> Setup took {elapsed}");
                eprintln!("> Waiting for tasks to finish.");
                eprintln!("  Press Ctrl-C to terminate.\x1b[A");
            }
            _ = watchdog(timeout), if setup_done.is_terminated() => {
                eprintln!("\x1b[2K");
                eprintln!("\x1b[31mTimeout\x1b[0m");
                break;
            }
            _ = ctrl_c() => {
                eprintln!("\x1b[2K");
                eprintln!("\x1b[31mCancelled\x1b[0m");
                break;
            }
            res = tracker.next() => {
                eprint!("\x1b[2K");
                if clear_line {
                    eprint!("\x1b[A\x1b[2K");
                }
                let Some(res): Option<Result<()>> = res else {
                    eprintln!();
                    break;
                };
                match res {
                    Ok(()) => {
                        incomplete -= 1;
                        success += 1;
                        clear_line = true;
                        eprintln!("> \x1b[32m{} .. [OK]\x1b[0m", count - tracker.len());
                        eprintln!("  Press Ctrl-C to terminate.\x1b[A");
                    }
                    Err(err) => {
                        incomplete -= 1;
                        failed += 1;
                        clear_line = false;
                        eprintln!("> \x1b[31m{} .. {err}\x1b[0m", count - tracker.len());
                        eprintln!("  Press Ctrl-C to terminate.\x1b[A");
                    }
                }
            }
        }
    }

    if success != count {
        println!(
            "\x1b[31m{success} tasks succeeded, {failed} tasks failed, {incomplete} tasks didn't finish\x1b[0m"
        );
        bail!("Some tasks did not succeed");
    }

    let elapsed = start.get().unwrap().elapsed();
    let throuput = count as f64 / elapsed.as_secs_f64();
    let duration = format_duration(elapsed);

    println!("\x1b[32m{success} tasks succeeded\x1b[0m");
    println!("\x1b[32m  elapsed time: {duration}\x1b[0m");
    println!("\x1b[32m  throuput: {throuput} tasks/s\x1b[0m");

    let shim = get_runtime(&shim_path).unwrap_or("unknown");
    let containerd_shim = if containerd { "containerd" } else { "mock" };

    if let Some(json_output) = json_output {
        let results = vec![BenchmarkResult {
            name: format!(
                "Stress Test Tasks Throughput with {} service - {}",
                containerd_shim, shim
            ),
            unit: "tasks/s".to_string(),
            value: throuput,
            extra: Some(format!(
                "Image: {}\nTasks: {}\nParallel: {}\nDuration: {}",
                image, count, parallel, duration
            )),
        }];
        serde_json::to_writer_pretty(&mut File::create(json_output)?, &results)?;
    }
    Ok(())
}
