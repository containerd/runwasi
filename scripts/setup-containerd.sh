#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

# Define variables
CONTAINERD_VERSION="1.7.22"
RUNC_VERSION="1.1.14"
CNI_PLUGINS_VERSION="1.5.1"

# Change to arm64 if running on apple silicon
CHIPSET="arm64"

# Define URLs
CONTAINERD_URL="https://github.com/containerd/containerd/releases/download/v${CONTAINERD_VERSION}/containerd-${CONTAINERD_VERSION}-linux-${CHIPSET}.tar.gz"
RUNC_URL="https://github.com/opencontainers/runc/releases/download/v${RUNC_VERSION}/runc.${CHIPSET}"
CNI_PLUGINS_URL="https://github.com/containernetworking/plugins/releases/download/v${CNI_PLUGINS_VERSION}/cni-plugins-linux-${CHIPSET}-v${CNI_PLUGINS_VERSION}.tgz"
CONTAINERD_SERVICE_URL="https://raw.githubusercontent.com/containerd/containerd/main/containerd.service"

# Install containerd
curl -LO $CONTAINERD_URL
sudo tar -C /usr/local -xzvf containerd-${CONTAINERD_VERSION}-linux-${CHIPSET}.tar.gz
rm -f containerd-${CONTAINERD_VERSION}-linux-${CHIPSET}.tar.gz

# Install runc
curl -LO $RUNC_URL
sudo install -m 755 runc.${CHIPSET} /usr/local/sbin/runc
rm -f runc.${CHIPSET}

# Install CNI plugins
curl -LO $CNI_PLUGINS_URL
sudo mkdir -p /opt/cni/bin
sudo tar -C /opt/cni/bin -xzvf cni-plugins-linux-${CHIPSET}-v${CNI_PLUGINS_VERSION}.tgz
rm -f cni-plugins-linux-${CHIPSET}-v${CNI_PLUGINS_VERSION}.tgz

# Create containerd default configuration
sudo mkdir -p /etc/containerd
containerd config default | sudo tee /etc/containerd/config.toml

# Modify the containerd config for systemd cgroup
sudo sed -i 's/SystemdCgroup = false/SystemdCgroup = true/g' /etc/containerd/config.toml

# Download and install the systemd service for containerd
sudo curl -L $CONTAINERD_SERVICE_URL -o /etc/systemd/system/containerd.service

# Reload systemd daemon and start containerd
sudo systemctl daemon-reload
sudo systemctl start containerd
sudo systemctl enable containerd

echo "containerd setup completed successfully!"
