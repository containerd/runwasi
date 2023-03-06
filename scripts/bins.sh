#!/usr/bin/env bash

# Get the list of binaries from the Cargo.toml file.
# If targeting a specific crate, pass the crate name as the first argument.

cargo metadata --format-version=1 | jq --arg CRATE "${1}" -f ./scripts/bins.jq
