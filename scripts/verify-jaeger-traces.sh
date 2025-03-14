#!/usr/bin/env bash
set -euo pipefail

TRACE_DATA=$(curl -s "http://localhost:16686/api/traces?service=containerd&limit=0" \
    | jq '[ .data[].spans[].operationName ]')

PREFIX="containerd_shimkit::sandbox"
REQUIRED_OPS=(
    "${PREFIX}::shim::local::create"
    "${PREFIX}::shim::local::wait"
    "${PREFIX}::shim::local::start"
    "${PREFIX}::shim::local::delete"
    "${PREFIX}::shim::local::state"
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