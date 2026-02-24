#!/bin/bash
# Use cargo-binstall to install cross from pre-built binaries
# This avoids the dependency resolution issue where cargo install
# resolves dependencies to latest versions that require rustc 1.88+

# Install cargo-binstall if not already installed
if ! command -v cargo-binstall &> /dev/null; then
    curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
fi

# Install cross using cargo-binstall (downloads pre-built binary)
# Use --force to ensure the binary is actually present even if metadata
# says it's already installed (the Rust cache may have cleaned the binary)
cargo binstall cross --version 0.2.5 --no-confirm --force

if [ ! -z "$CI" ]; then

    echo "CARGO=cross" >> ${GITHUB_ENV}

    # See https://github.com/containerd/runwasi/pull/813#issuecomment-2619138618
    echo "CROSS_NO_WARNINGS=0" >> ${GITHUB_ENV}

    if [ ! -z "$1" ]; then
        echo "TARGET=$1" >> ${GITHUB_ENV}
    fi

fi
