mod mocks;
mod protos;
mod utils;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use mocks::Task;
use tokio_util::task::TaskTracker;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    verbose: bool,

    shim: PathBuf,

    #[arg(short, long)]
    serial: bool,

    #[arg(short('n'), long, default_value("10"))]
    count: u32,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    env_logger::try_init()?;

    let Cli {
        shim,
        verbose,
        serial,
        count,
    } = Cli::parse();
    let shim: String = shim.to_string_lossy().into_owned();

    let c8d = mocks::Containerd::new(verbose).await?;

    let shim = c8d.start_shim(shim).await?;

    let mut tasks = vec![];
    for _ in 0..count {
        let task = shim.task().await?;
        tasks.push(task);
    }

    let tracker = TaskTracker::new();
    for task in tasks {
        if serial {
            run_task(task).await;
        } else {
            tracker.spawn(run_task(task));
        }
    }

    tracker.close();
    tracker.wait().await;

    shim.shutdown().await?;
    c8d.shutdown().await
}

async fn run_task(task: Task) {
    async move {
        task.create().await?;
        task.start().await?;
        task.wait().await?;
        task.delete().await?;
        Ok(())
    }
    .await
    .inspect_err(|e: &anyhow::Error| eprintln!("{e}"))
    .unwrap_or_default();
}
