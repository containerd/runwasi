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
- run `make test/out/img.tar`,
- open a secondary terminal and run `containerd`, and
- run `make load`.

After this, you can execute an example, like: `ctr run --rm --runtime=io.containerd.wasmtime.v1 docker.io/library/wasmtest:latest testwasm`.

> To kill the process from the example, you can run: `ctr kill -s SIGKILL testwasm`.

