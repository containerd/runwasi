use std::path::PathBuf;
use std::{sync::Arc, time::Instant};
use clap::Parser;
use containerd_client as client;
use client::{
    services::v1::{
        container::Runtime, containers_client::ContainersClient, tasks_client::TasksClient,
        Container, CreateContainerRequest, CreateTaskRequest, DeleteContainerRequest,
        DeleteTaskRequest, StartRequest, WaitRequest,
    },
    with_namespace,
};
use oci_spec::runtime::{ProcessBuilder, SpecBuilder, UserBuilder};
use prost_types::Any;
use tokio::sync::{mpsc, Semaphore};
use anyhow::{Result, Context};
use tonic::{Request, transport::Channel};

#[derive(Parser, Debug)]
#[command(name = "wasm-stress-test")]
#[command(about = "Stress test for WASM containers using containerd")]
struct Args {
    #[arg(long, default_value_t = false)]
    verbose: bool,
    
    #[arg(long, default_value_t = true)]
    container_output: bool,
    
    #[arg(long, default_value_t = 32)]
    parallel: usize,
    
    #[arg(long, default_value_t = 1000)]
    count: usize,
}

const SOCKET_PATH: &str = "/run/containerd/containerd.sock";
const NAMESPACE: &str = "default";
const RUNTIME_NAME: &str = "io.containerd.wasmtime.v1";
const WASM_IMAGE: &str = "ghcr.io/containerd/runwasi/wasi-demo-oci:latest";

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    if !args.verbose {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .init();
    }

    let channel = client::connect(SOCKET_PATH)
        .await
        .context("Failed to connect to containerd")?;

    let max_parallel = if args.parallel == 0 {
        args.count
    } else {
        args.parallel
    };

    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let (error_tx, mut error_rx) = mpsc::channel(args.count);
    let start_time = Instant::now();

    let mut handles = Vec::new();

    for i in 0..args.count {
        let channel = channel.clone();
        let semaphore = semaphore.clone();
        let error_tx = error_tx.clone();

        let handle = tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            
            let container_id = format!("stress-test-{}-{}", i, std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos());

            if let Err(e) = run_container(
                channel.clone(),
                &container_id,
            ).await {
                let _ = error_tx.send(format!(
                    "Failed to run container {}: {}", container_id, e
                )).await;
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }
    drop(error_tx);

    let duration = start_time.elapsed();
    println!("\nStress test completed in {:?}", duration);
    println!("Containers created: {}", args.count);
    println!("Concurrent workers: {}", max_parallel);

    let mut error_count = 0;
    while let Some(error) = error_rx.recv().await {
        error_count += 1;
        if args.verbose {
            eprintln!("{}", error);
        }
    }

    println!("\nTotal errors: {}", error_count);
    println!("Success rate: {:.2}%", 
        (args.count as f64 - error_count as f64) / args.count as f64 * 100.0);

    Ok(())
}

async fn run_container(
    channel: Channel,
    container_id: &str,
) -> Result<()> {
    let mut containers_client = ContainersClient::new(channel.clone());
    let process = ProcessBuilder::default()
        .user(UserBuilder::default().build().unwrap())
        .args(vec!["wasi-demo-oci.wasm".into(), "echo".into(), "hello".into()])
        .cwd(PathBuf::from("/"))
        .build()
        .unwrap();

    let spec = SpecBuilder::default()
        .version("1.1.0")
        .process(process)
        .build()
        .unwrap();

    let spec: Any = Any {
        type_url: "types.containerd.io/opencontainers/runtime-spec/1/Spec".to_string(),
        value: serde_json::to_vec(&spec).unwrap(),
    };

    let container = Container {
        id: container_id.to_string(),
        image: WASM_IMAGE.to_string(),
        runtime: Some(Runtime {
            name: RUNTIME_NAME.to_string(),
            options: None,
        }),
        spec: Some(spec),
        ..Default::default()
    };

    let req = CreateContainerRequest {
        container: Some(container),
    };
    let req = with_namespace!(req, NAMESPACE);
    let _resp = containers_client
        .create(req)
        .await
        .expect("Failed to create container");

    let mut client = TasksClient::new(channel.clone());

    let req = CreateTaskRequest {
        container_id: container_id.to_string(),
        ..Default::default()
    };
    let req = with_namespace!(req, NAMESPACE);

    let _resp = client.create(req).await.expect("Failed to create task");
    
    let req = StartRequest {
        container_id: container_id.to_string(),
        ..Default::default()
    };
    let req = with_namespace!(req, NAMESPACE);

    match client.start(req).await {
        Ok(_) => Ok(()),
        Err(e) => {
            let cleanup_req = Request::new(DeleteTaskRequest {
                container_id: container_id.to_string(),
            });
            let _ = client.delete(cleanup_req).await;
            let cleanup_req = Request::new(DeleteContainerRequest {
                id: container_id.to_string(),
            });
            let _ = containers_client.delete(cleanup_req).await;
            Err(anyhow::anyhow!("Failed to start container: {}", e))
        }
    }?;

    let req = WaitRequest {
        container_id: container_id.to_string(),
        ..Default::default()
    };
    let req = with_namespace!(req, NAMESPACE);

    client.wait(req).await?;

    let req = DeleteTaskRequest {
        container_id: container_id.to_string(),
    };
    let req = with_namespace!(req, NAMESPACE);

    client.delete(req).await?;

    let req = DeleteContainerRequest {
        id: container_id.to_string(),
    };
    let req = with_namespace!(req, NAMESPACE);

    containers_client.delete(req).await?;

    Ok(())
}