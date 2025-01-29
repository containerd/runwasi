# Windows: Getting Started

Currently, **runwasi** depends on a Linux environment (i.e., because it has to wire up networking and rootfs mounts). Therefore, to run it on Windows, we recommend utilizing the Windows Subsystem for Linux (WSL).

To get started with WSL, see [this](https://docs.microsoft.com/en-us/windows/wsl/install).

Once you have your WSL environment set and you have cloned the **runwasi** repository, you will need to install Docker and the Docker Buildx plugin.

To install Docker and the Docker Buildx Plugin, see [this](https://docs.docker.com/engine/install/) to find specific installation instructions for your WSL distro.

Before proceeding, it's also recommended to install Docker Desktop on Windows and run it once.

To finish off installing pre-requisites, install Rust following [this](https://www.rust-lang.org/tools/install).

After following these steps and navigating to the runwasi directory in your terminal:
- run `make build`,
- run `make install`,
- run `make pull-app`.

After this, you can execute an example, like: `ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm`.

> To kill the process from the example, you can run: `ctr task kill -s SIGKILL testwasm`.

## Building and developing on Windows

You need to install `wasmedge`, `llvm` and `make`. This can be done using `winget`, `choco` or manually. (note as of writing this `winget` doesn't have the latest package and will builds will fail).  See `.github/scripts/build-windows.sh` for an example.

Once you have those dependencies you will need to set env:

```
$env:WASMEDGE_LIB_DIR="C:\Program Files\WasmEdge\lib"
$env:WASMEDGE_INCLUDE_DIR="C:\Program Files\WasmEdge\include"    
```

Then you can run:

```
make build
```

### Using VS code
If you are using VS Code for development you can use the following `settings.json` in the `.vscode` folder of the project:

```
{
    "rust-analyzer.cargo.noDefaultFeatures": true,
    "rust-analyzer.cargo.extraEnv": {
        "WASMEDGE_LIB_DIR": "C:\\Program Files\\WasmEdge\\lib",
        "WASMEDGE_INCLUDE_DIR": "C:\\Program Files\\WasmEdge\\include"
    }
}
```