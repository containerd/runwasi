#!/usr/bin/env bash
set -euo pipefail

TRACE_DATA=$(curl -s "http://localhost:16686/api/traces?service=containerd&limit=0" \
    | jq '[ .data[].spans[].operationName ]')

REQUIRED_OPS=("task_create" "task_wait" "task_start" "task_delete" "task_state" "wait" "shutdown" "shim_main_inner")

for op in "${REQUIRED_OPS[@]}"; do
  COUNT=$(echo "$TRACE_DATA" | jq --arg op "$op" '[ .[] | select(. == $op) ] | length')
  if [ "$COUNT" -eq 0 ]; then
    echo "Operation '$op' not found in Jaeger!"
    exit 1
  fi
done

echo "All required operations found in Jaeger!"