#!/bin/bash

# Usage: parse_ref.sh <ref|crate|runtime>
#
# This scripts parses a tag ref of the type
#  refs/tags/<crate>/v<version>
# and prints
#  CRATE=<crate>
#  VERSION=v<version>
#  RUNTIME=<runtime>
# with <crate> = containerd-shim-<runtime>
# The script errors if the crate or the version can't be parsed,
# or the version doesn't match the value in `crates/<crate>/Cargo.toml`.
#
# If <ref> provided, that value is parsed.
# If <crate> is provided, the most recent tag matching `<crate>/*` is parsed.
# If <runtime> is provided, the most recent tag matching `containerd-shim-<runtime>/*` is parsed.
# If no argument is provided, it defaults to `containerd-shim-wasm`.

set -e

if [ ! "${1}" = "${1#refs/*/}" ]; then
REF="$1"
else
REF="${1#containerd-shim-}"
REF="containerd-shim-${REF:-wasm}"
REF="refs/tags/$(git describe --tags --abbrev=0 --match "${REF}/*")"
fi

CRATE="$(cut -d/ -f1 <<<"${REF#refs/*/}")"
VERSION="$(cut -d/ -f2 <<<"${REF#refs/*/}")"
RUNTIME="${CRATE#containerd-shim-}"
TOMLVER="$(./scripts/version.sh "${CRATE}")"

echo "CRATE=${CRATE}"
echo "VERSION=${VERSION}"
echo "RUNTIME=${RUNTIME}"
echo "REF=${REF}"

if [ -z "${CRATE}" ]; then
echo "::error::Could not determine crate name from ref '${REF}'" >&2
exit 1
fi

if [ -z "${VERSION}" ]; then
echo "::error::Could not determine version from ref '${REF}'" >&2
exit 1
fi

if [ ! "${VERSION}" = "v${TOMLVER}" ]; then
echo "::error::Version mismatch: tag version ${VERSION} != crate's Cargo.toml version v${TOMLVER}" >&2
exit 1
fi