#!/usr/bin/env bash

set -e

cargo build --all --verbose --features generate_doc
git status --porcelain | grep README.md || exit 0

echo "README.md is not up to date. Please run 'cargo build --all --features generate_doc' and commit the changes." >&2
exit 1
