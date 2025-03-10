# Quickstart with Kubernetes

This guide will walk you through running WebAssembly workloads on Kubernetes using Runwasi.

## Prerequisites

Before getting started, ensure you have:
- Installed Runwasi shims as described in the [Installation Guide](./installation.md)
- Basic familiarity with Kubernetes concepts
- Access to a Kubernetes cluster or the ability to create one using Kind or k3s

## Setting Up Kubernetes for WebAssembly

Runwasi enables WebAssembly workloads to run on Kubernetes by providing a containerd shim that interfaces with the Kubernetes container runtime interface (CRI). You can use either Kind or k3s for a local development environment.

### Option 1: Using Kind

[Kind](https://kind.sigs.k8s.io/) (Kubernetes IN Docker) is a tool for running local Kubernetes clusters using Docker containers as nodes.

1. Install and configure dependencies:
```bash
curl -Lo ./kind https://kind.sigs.k8s.io/dl/v0.27.0/kind-linux-amd64
chmod +x ./kind
sudo mv ./kind /usr/local/bin/

# Build and install the Wasmtime shim if you haven't already
make build-wasmtime
sudo make install-wasmtime
```

2. Create a Kind configuration file:
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

### Option 2: Using k3s

[k3s](https://k3s.io/) is a lightweight, certified Kubernetes distribution designed for edge, IoT, CI, and development use cases.

1. Install k3s and build the shim:
```bash
curl -sfL https://get.k3s.io | sh -

# Build and install the Wasmtime shim if you haven't already
make build-wasmtime
sudo make install-wasmtime
```

2. Configure k3s to use the WebAssembly runtime:
```bash
sudo mkdir -p /var/lib/rancher/k3s/agent/etc/containerd/

cat << EOF | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]
  runtime_type = "io.containerd.wasmtime.v1"
EOF

sudo systemctl restart k3s
```

## Deploying WebAssembly Workloads

After setting up your Kubernetes cluster with Runwasi, you can deploy WebAssembly workloads.

1. Create a deployment YAML file:

```yaml
# deploy.yaml
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
  replicas: 1
  selector:
    matchLabels:
      app: wasi-demo
  template:
    metadata:
      labels:
        app: wasi-demo
    spec:
      runtimeClassName: wasm
      containers:
      - name: demo
        image: ghcr.io/containerd/runwasi/wasi-demo-app:latest
```

2. Apply the deployment with Kind:
```bash
kubectl --context kind-runwasi-cluster apply -f deploy.yaml
```

   Or with k3s:
```bash
sudo k3s kubectl apply -f deploy.yaml
```

3. Check the status of your deployment:

   With Kind:
```bash
kubectl --context kind-runwasi-cluster get pods
kubectl --context kind-runwasi-cluster logs -l app=wasi-demo
```

   With k3s:
```bash
sudo k3s kubectl get pods
sudo k3s kubectl logs -l app=wasi-demo
```

You should see output like:
```
This is a song that never ends.
Yes, it goes on and on my friends.
Some people started singing it not knowing what it was,
So they'll continue singing it forever just because...
```

## Using Other WebAssembly Runtimes

You can use different WebAssembly runtimes by changing the runtime type in your containerd configuration:

- For WasmEdge:
```
runtime_type = "io.containerd.wasmedge.v1"
```

- For Wasmer:
```
runtime_type = "io.containerd.wasmer.v1"
```

Make sure you've installed the corresponding shim binary.

## Cleaning Up

To remove your test deployment:

With Kind:
```bash
kubectl --context kind-runwasi-cluster delete -f deploy.yaml
kind delete cluster --name runwasi-cluster
```

With k3s:
```bash
sudo k3s kubectl delete -f deploy.yaml
# Optionally uninstall k3s
/usr/local/bin/k3s-uninstall.sh
```

## Next Steps

Now that you've set up Kubernetes to run WebAssembly workloads:

- Learn about [OCI Integration](../oci-decision-flow.md) for container images
- Explore [Architecture Overview](../developer/architecture.md) to understand how Runwasi works
- Check out [OpenTelemetry Integration](../opentelemetry.md) for monitoring your WebAssembly workloads
