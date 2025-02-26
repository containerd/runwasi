<picture>
  <source media="(prefers-color-scheme: dark)" srcset="./art/logo/runwasi_icon3.svg">
  <img alt="runwasi logo" src="./art/logo/runwasi_icon1.svg">
</picture>

# runwasi

This is a project to facilitate running wasm workloads managed by containerd either directly (ie. through ctr) or as directed by Kubelet via the CRI plugin.
It is intended to be a (rust) library that you can take and integrate with your wasm host.
Included in the repository is a PoC for running a plain wasi host (ie. no extra host functions except to support wasi system calls).

## Community

- If you haven't joined the CNCF slack yet, you can do so [here](https://slack.cncf.io/).
- Come join us on our [slack channel #runwasi](https://cloud-native.slack.com/archives/C04LTPB6Z0V)
on the CNCF slack.
- Public Community Call on Tuesdays every other week at 9:00 AM PT: [Zoom](https://zoom.us/my/containerd?pwd=bENmREpnSGRNRXdBZWV5UG8wbU1oUT09), [Meeting Notes](https://docs.google.com/document/d/1aOJ-O7fgMyRowHD0kOoA2Z_4d19NyAvvdqOkZO3Su_M/edit?usp=sharing)

## Usage

runwasi is intended to be consumed as a library to be linked to from your own wasm host implementation. It creates one shim process per container or k8s pod.

You need to implement a trait to teach runwasi how to use your wasm host.

There are two ways to do this:
* implementing the `sandbox::Instance` trait
* or implementing the `container::Engine` trait

The most flexible but complex is the `sandbox::Instance` trait:

```rust
pub trait Instance {
    /// The WASI engine type
    type Engine: Send + Sync + Clone;

    /// Create a new instance
    fn new(id: String, cfg: &InstanceConfig) -> Self;
    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, Error>;
    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error>;
    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), Error>;
    /// Wait for the instance to exit
    /// The waiter is used to send the exit code and time back to the caller
    /// Ideally this would just be a blocking call with a normal result, however
    /// because of how this is called from a thread it causes issues with lifetimes of the trait implementer.
    fn wait(&self, waiter: &Wait) -> Result<(), Error>;
}
```

The `container::Engine` trait provides a simplified API:

```rust
pub trait Engine: Clone + Send + Sync + 'static {
    /// The name to use for this engine
    fn name() -> &'static str;
    /// Run a WebAssembly container
    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32>;
    /// Check that the runtime can run the container.
    /// This checks runs after the container creation and before the container starts.
    /// By default it checks that the wasi_entrypoint is either:
    /// * a OCI image with wasm layers
    /// * a file with the `wasm` filetype header
    /// * a parsable `wat` file.
    fn can_handle(&self, ctx: &impl RuntimeContext) -> Result<()> { /* default implementation*/ }
}
```

After implementing `container::Engine` you can use `container::Instance<impl container::Engine>`, which implements the `sandbox::Instance` trait.

To use your implementation in "normal" mode, you'll need to create a binary which has a main that looks something like this:

```rust
use containerd_shim as shim;
use containerd_shim_wasm::sandbox::{ShimCli, Instance}

struct MyInstance {
    // ...
}

impl Instance for MyInstance {
    // ...
}

fn main() {
    shim::run::<ShimCli<MyInstance>>("io.containerd.myshim.v1", opts);
}
```

or when using the `container::Engine` trait, like this:

```rust
use containerd_shim as shim;
use containerd_shim_wasm::{sandbox::ShimCli, container::{Instance, Engine}}

struct MyEngine {
    // ...
}

impl Engine for MyEngine {
    // ...
}

fn main() {
    shim::run::<ShimCli<Instance<MyEngine>>>("io.containerd.myshim.v1", opts);
}
```

Note you can implement your own ShimCli if you like and customize your wasm engine and other things.
I encourage you to checkout how that is implemented.

The shim binary just needs to be installed into `$PATH` (as seen by the containerd process) with a binary name like `containerd-shim-myshim-v1`.

Check out these projects that build on top of runwasi:
- [spinkube/containerd-shim-spin](https://github.com/spinkube/containerd-shim-spin)
- [deislabs/containerd-wasm-shims](https://github.com/deislabs/containerd-wasm-shims)


### Components

- **containerd-shim-[ wasmedge | wasmtime | wasmer | wamr ]-v1**

This is a containerd shim which runs wasm workloads in [WasmEdge](https://github.com/WasmEdge/WasmEdge) or [Wasmtime](https://github.com/bytecodealliance/wasmtime) or [Wasmer](https://github.com/wasmerio/wasmer).
You can use it with containerd's `ctr` by specifying `--runtime=io.containerd.[ wasmedge | wasmtime | wasmer | wamr ].v1` when creating the container.
And make sure the shim binary must be in $PATH (that is the $PATH that containerd sees). Usually you just run `make install` after `make build`.
> build shim with wasmedge we need install library first

This shim runs one per pod.

## Demo

### Installing the shims for use with Containerd

Make sure you have [installed dependencies](./CONTRIBUTING.md#setting-up-your-local-environment) and install the shims:

```terminal
make build
sudo make install
```

> Note: `make build` will only build one binary. The `make install` command copies the binary to $PATH and uses symlinks to create all the component described above.

Pull the test image:

```
make pull-app
```

### Demo 1 using container image that contains a Wasm module.

Run it with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime | wasmer | wamr ].v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm /wasi-demo-app.wasm echo 'hello'`. You should see some output repeated like:

```terminal
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-app:latest testwasm

This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...

This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...

(...)
```

To kill the process, you can run in other session: `sudo ctr task kill -s SIGKILL testwasm`.

The test binary supports commands for different type of functionality, check [crates/wasi-demo-app/src/main.rs](crates/wasi-demo-app/src/main.rs) to try it out.

### Demo 2 using OCI Images with custom WASM layers

The previous demos run with an OCI Container image containing the wasm module in the file system.  Another option is to provide a cross-platform OCI Image that that will not have the wasm module or components in the file system of the container that wraps the wasmtime/wasmedge process.  This OCI Image with custom WASM layers can be run across any platform and provides for de-duplication in the Containerd content store among other benefits. To build OCI images using your own images you can use the [oci-tar-builder](./crates/oci-tar-builder/README.md)

To learn more about this approach checkout the [design document](https://docs.google.com/document/d/11shgC3l6gplBjWF1VJCWvN_9do51otscAm0hBDGSSAc/edit).

> **Note**: This requires containerd 1.7.7+ and 1.6.25+.  If you do not have these patches for both `containerd` and `ctr` you will end up with an error message such as `mismatched image rootfs and manifest layers` at the import and run steps. Latest versions of k3s and kind have the necessary containerd versions.

Pull the OCI image with WASM layers image:

```
make pull
```

Run the image with `sudo ctr run --rm --runtime=io.containerd.[ wasmedge | wasmtime | wasmer | wamr ].v1 ghcr.io/containerd/runwasi/wasi-demo-oci:latest testwasmoci`

```
sudo ctr run --rm --runtime=io.containerd.wasmtime.v1 ghcr.io/containerd/runwasi/wasi-demo-oci:latest testwasmoci wasi-demo-oci.wasm echo 'hello'
hello
exiting
```

### Demo 3 using Wasm OCI Artifact

The [CNCF tag-runtime wasm working group](https://tag-runtime.cncf.io/wgs/wasm/charter/) has a [OCI Artifact format for Wasm](https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/).  This is a new Artifact type that enable the usage across projects beyond just runwasi, see the https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/#implementations

```
make test/k8s-oci-wasmtime
```

> note: We are using a kubernetes cluster to run here since containerd's ctr has a bug that results in ctr: `unknown image config media type application/vnd.wasm.config.v0+json`

### Demo 4: Running on Kubernetes

You can run WebAssembly workloads on Kubernetes using either Kind or k3s.

#### Using Kind

1. Install and configure dependencies:
```bash
curl -Lo ./kind https://kind.sigs.k8s.io/dl/v0.21.0/kind-linux-amd64
chmod +x ./kind
sudo mv ./kind /usr/local/bin/

make build-wasmtime
sudo make install-wasmtime
```

2. Create a Kind configuration:
```yaml
# kind-config.yaml
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
name: runwasi-cluster
nodes:
- role: control-plane
  extraMounts:
  - hostPath: /usr/local/bin/containerd-shim-wasmtime-v1
    containerPath: /usr/local/bin/containerd-shim-wasmtime-v1
```

3. Create and configure the cluster:
```bash
kind create cluster --name runwasi-cluster --config kind-config.yaml

kubectl cluster-info --context kind-runwasi-cluster

cat << EOF | docker exec -i runwasi-cluster-control-plane tee /etc/containerd/config.toml
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]
  runtime_type = "io.containerd.wasmtime.v1"
EOF

docker exec runwasi-cluster-control-plane systemctl restart containerd
```

4. Deploy the demo application:
```bash
kubectl --context kind-runwasi-cluster apply -f test/k8s/deploy.yaml
```

5. Check the logs:
```bash
kubectl --context kind-runwasi-cluster logs -l app=wasi-demo
```
where you should see the output of the demo application:

```console
This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...
```

#### Using k3s

1. Install k3s and build the shim:
```bash
curl -sfL https://get.k3s.io | sh -

make build-wasmtime
sudo make install-wasmtime
```

2. Configure k3s to use the Wasm runtime:
```bash
sudo mkdir -p /var/lib/rancher/k3s/agent/etc/containerd/

cat << EOF | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]
  runtime_type = "io.containerd.wasmtime.v1"
EOF

sudo systemctl restart k3s
```

3. Deploy the demo application:
```bash
sudo k3s kubectl apply -f test/k8s/deploy.yaml
```

4. Check the deployment:
```bash
sudo k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=90s

sudo k3s kubectl get pods
sudo k3s kubectl logs -l app=wasi-demo
```

You should see "This is a song that never ends." repeated in the logs.

5. Clean up when done:
```bash
sudo k3s kubectl delete -f test/k8s/deploy.yaml

# Optionally uninstall k3s
/usr/local/bin/k3s-uninstall.sh
```

#### The `deploy.yaml` file

The deployment includes:
```yaml
apiVersion: node.k8s.io/v1
kind: RuntimeClass
metadata:
  name: wasm
handler: wasm
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: wasi-demo
spec:
  # ...
  template:
    spec:
      runtimeClassName: wasm # Use the wasm runtime class
      containers:
      - name: demo
        image: ghcr.io/containerd/runwasi/wasi-demo-app:latest
```


To see demos for other runtimes, replace `wasmtime` with `wasmedge`, `wasmer`, or `wamr` in the above commands.

In addition, check out the [Kubernetes + Containerd + Runwasi](https://wasmedge.org/docs/develop/deploy/kubernetes/kubernetes-containerd-runwasi) for more on how to run WasmEdge on Kubernetes.


### WASI/HTTP Demo for `wasmtime-shim`
See [wasmtime-shim documentation](./crates/containerd-shim-wasmtime/README.md#WASI/HTTP).


## Contributing

To begin contributing, learn to build and test the project or to add a new shim please read our [CONTRIBUTING.md](./CONTRIBUTING.md)
