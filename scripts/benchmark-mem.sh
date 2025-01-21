#!/bin/bash

set -euxo pipefail

# Parse CLI arguments
RUNTIME=${1:-"wasmtime"}; shift || true
IMAGE=${1:-"ghcr.io/containerd/runwasi/wasi-demo-app:latest"}; shift || true

if [ $IMAGE == "ghcr.io/containerd/runwasi/wasi-demo-app:latest" ] && [ "$#" == "0" ]; then
    set -- /wasi-demo-app.wasm echo 'hello'
fi

# Run the shim and collect logs
LOG_FILE=$(mktemp)
journalctl -fn 0 -u containerd | timeout -k 16s 15s grep -m 2 'peak memory usage was' > $LOG_FILE &
ctr run --null-io --rm --runtime=io.containerd.$RUNTIME.v1 "$IMAGE" testwasm "$@"

# Parse the logs
wait
SHIM_MEM=$(cat $LOG_FILE | grep 'Shim peak memory usage was' | sed -E 's/.*peak resident set ([0-9]+) kB.*/\1/')
ZYGOTE_MEM=$(cat $LOG_FILE | grep 'Zygote peak memory usage was' | sed -E 's/.*peak resident set ([0-9]+) kB.*/\1/')
rm $LOG_FILE

if [ "$SHIM_MEM" == "" ] || [ "$ZYGOTE_MEM" == "" ]; then
    exit 1
fi

# Print the JSON for the benchmark report
cat <<EOF
[
    {
        "name": "Shim memory usage",
        "unit": "kB",
        "value": $SHIM_MEM
    },
    {
        "name": "Zygote memory usage",
        "unit": "kB",
        "value": $ZYGOTE_MEM
    }
]
EOF