#!/bin/sh

# Entrypoint for the k3s test container.
# Intended to be run using `e2e-k3s-runner`.
#
# Usage:
#   /entry.sh <shim> <deployment>
# where <shim> is the file name to a containerd shim binary
# and <deployment> is the name of the deplotment to test.
#
# Additionally this this scrits also expects the
# following files in the filesystem
#  * /shim/<shim>        The shim binary
#  * /image/img.tar      OCI image
#  * /deploy/deploy.yml  k8s deployment config
#
# The script will place all the generated logs in /var/log

set -e

SHIM="$1"
DEPLOYMENT="$2"

# Print the steps
set -x

# Start the k3s server
k3s server \
    --log /var/log/k3s.log \
    --disable traefik \
    &

# Import the image, and deploy
ctr image import --all-platforms "/image/img.tar"
kubectl apply -f /deploy/deploy.yaml

# Wait for the test condition to be met
if ! kubectl wait deployment "${DEPLOYMENT}" --for condition=Available=True --timeout=120s; then
    kubectl get pods --all-namespaces -o wide
    kubectl describe pods --all-namespaces
    exit 1
else
    kubectl get pods --all-namespaces -o wide
fi

# With certain types of bugs, the container errors after having started.
# An example is when the network namespace is not correctly set up, the
# nginx sidecar container starts, but it errors to bind on the TCP port.
# When this happens, the pod enters a crash loop.
# To detect these situations, we wait some time, and check that the pods
# are still running
sleep 10s
if ! kubectl wait deployment "${DEPLOYMENT}" --for condition=Available=True --timeout=0s; then
    kubectl get pods --all-namespaces -o wide
    kubectl describe pods --all-namespaces
    exit 1
else
    kubectl get pods --all-namespaces -o wide
fi
