#!/bin/bash
set -ex

rm -f /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
cp -f /var/lib/rancher/k3s/agent/etc/containerd/config.toml /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
cat <<EOF >> /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]
  runtime_type = "$PWD/dist/bin/containerd-shim-$1-v1"
EOF

cat <<EOF > /etc/systemd/system/k3s-runwasi.service.env
CONTAINERD_NAMESPACE='${CONTAINERD_NAMESPACE:-default}'
NO_PROXY=192.168.0.0/16
EOF

systemctl daemon-reload
systemctl restart k3s-runwasi
while ! bin/k3s ctr version; do sleep 1; done
bin/k3s ctr image import --all-platforms $2
while [ "$(bin/k3s kubectl get pods --all-namespaces --no-headers | wc -l)" == "0" ]; do sleep 1; done
while [ "$(bin/k3s kubectl get pods --all-namespaces --no-headers | grep -vE "Completed|Running" | wc -l)" != "0" ]; do sleep 1; done
