#!/usr/bin/env bash
set -euo pipefail

TRACE_DATA=$(curl -s "http://localhost:16686/api/traces?service=containerd&limit=0" \
    | jq '[ .data[].spans[].operationName ]')

PREFIX="containerd_shim_wasm::sandbox"
REQUIRED_OPS=(
    "${PREFIX}::shim::local::task_create"
    "${PREFIX}::shim::local::task_wait"
    "${PREFIX}::shim::local::task_start"
    "${PREFIX}::shim::local::task_delete"
    "${PREFIX}::shim::local::task_state"
    "${PREFIX}::shim::cli::wait"
    "${PREFIX}::shim::local::shutdown"
    "${PREFIX}::cli::shim_main_inner"
)

for op in "${REQUIRED_OPS[@]}"; do
  COUNT=$(echo "$TRACE_DATA" | jq --arg op "$op" '[ .[] | select(. == $op) ] | length')
  if [ "$COUNT" -eq 0 ]; then
    echo "Operation '$op' not found in Jaeger!"
    exit 1
  fi
done

echo "All required operations found in Jaeger!"