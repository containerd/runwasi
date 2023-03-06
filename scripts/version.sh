#!/usr/bin/env bash

# Get the list of binaries from the Cargo.toml file.
# If targeting a specific crate, pass the crate name as the first argument.

read -r -d '' Q <<-'EOF'
include "crates";
.packages | filter_by_package($CRATE)[0].version
EOF

set -u -e -o pipefail

cargo metadata --format-version=1 --no-deps | jq -r -L "${BASH_SOURCE[0]%/*}" --arg CRATE "${1}" "${Q}"
