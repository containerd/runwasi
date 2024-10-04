#!/bin/bash
# Inspired by https://stackoverflow.com/questions/40450238/parse-a-changelog-and-extract-changes-for-a-version
# This script will extract the changelog for a specific version from the CHANGELOG.md file
# Usage: ./extract-changelog.sh <version>
version=$1

awk -v ver="$version" '
/^## \[.*\]/ {
  if (p) exit
  if ($0 ~ "^## \\[" ver "\\]") { p=1; next }
}
p' crates/containerd-shim-wasm/CHANGELOG.md