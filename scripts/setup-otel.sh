#!/bin/bash

# start jeager endpoint 
docker run -d -p16686:16686 -p4317:4317 -p4318:4318 -e COLLECTOR_OTLP_ENABLED=true jaegertracing/all-in-one:latest

systemctl stop containerd

mkdir -p /etc/systemd/system/containerd.service.d

# Add the environment variable to the override file
cat <<EOF > /etc/systemd/system/containerd.service.d/override.conf
[Service]
Environment="OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318"
Environment="OTEL_SERVICE_NAME=wasmtime"
EOF

systemctl daemon-reload
systemctl restart containerd