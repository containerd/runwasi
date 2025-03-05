# Troubleshooting

This guide helps you troubleshoot common issues with runwasi shims.

## Container Fails to Start

If your wasm container fails to start, you might see events like these:

```
Events:
  Type     Reason                  Age               From               Message
  ----     ------                  ----              ----               -------
  Normal   Scheduled               18s               default-scheduler  Successfully assigned default/wasi-demo-75d6bb666c-479km to minikube
  Warning  FailedCreatePodSandBox  4s (x2 over 18s)  kubelet            Failed to create pod sandbox: rpc error: code = Unknown desc = failed to get sandbox runtime: no runtime for "wasmtime" is configured
```

#### Investigation Steps:

1. **Check runtime class configuration**: Ensure your `RuntimeClass` is correctly configured.
   ```bash
   kubectl get runtimeclass

   # Example output:
    NAME                  HANDLER               AGE
    wasm                  wasm                  8d
    wasmedge              wasmedge              8d
    wasmer                wasmer                8d
    wasmtime              wasmtime              8d
   ```
   
   > RuntimeClass in Kubernetes defines a handler name (e.g., `wasmtime`) that maps directly to a CRI section in containerd's config.toml (`[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasmtime]`), which then points to the shim binary (`containerd-shim-wasmtime-v1`) that executes your Wasm workload. See more details in [Runtime classes from containerd](https://github.com/containerd/containerd/blob/main/docs/cri/config.md#runtime-classes)


2. **Check containerd configuration**: Verify that the containerd configuration has a runtime configured for your Wasm shim (e.g., wasmtime).
   ```bash
   cat /etc/containerd/config.toml
   
   # Look for a section like:
   [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasmtime]
      runtime_type = "io.containerd.wasmtime.v1"
   ```

3. Make sure the containerd shim binary is in the PATH.

## Log-Based Troubleshooting

Examining logs is one of the most effective ways to troubleshoot issues in runwasi shims.

### Finding containerd Logs

Logs can be in different locations depending on your system:

#### Kubernetes Distributions
- k3s: `/var/lib/rancher/k3s/agent/containerd/containerd.log`
- k8s (kubeadm): `/var/log/containerd/containerd.log`

#### Using journalctl (systemd-based systems)
```bash
# View all containerd logs
journalctl -u containerd

# Follow containerd logs
journalctl -u containerd -f
```

### Kubernetes Pod Logs

In addition to containerd logs, Kubernetes pod logs show the logs from your containers.

```bash
kubectl logs <pod-name>

# Or stream logs in real-time
kubectl logs -f <pod-name>
```

### Using Structured Logging with Container and Pod IDs

runwasi shims include structured logging that automatically adds container ID and pod ID to log messages, making it easier to filter logs.

#### Finding Container ID and Pod ID:

```bash
# Get container IDs for a pod
kubectl get pod <pod-name> -o jsonpath='{.status.containerStatuses[*].containerID}'
# Output: containerd://<container-id>

# Get pod ID for a specific pod
crictl pods --name <pod-name> -q
```

Note: If you are using k3s, you can use the following command to get the pod ID:

```bash
k3s crictl pods --name <pod-name> -q
```

#### Filtering Logs by Container ID (k3s)

```bash
grep 'instance="<container-id>"' /var/lib/rancher/k3s/agent/containerd/containerd.log

# Example output:
time="2025-03-05T21:49:01.527630395Z" level=info instance="820f78385e9c29cbd6a0b6767619286ffd7a3384959ce909063a22041d17c718" pod="22a0daacfeabe74d165552794ec9615c67545566bec282d3fe0b5cc910e9cdb5" msg="setting up wasi"
```

#### Filtering Logs by Pod ID (k3s)

```bash
grep 'pod="22a0daacfeabe74d"' /var/lib/rancher/k3s/agent/containerd/containerd.log
```
