#!/bin/bash
# Install cross from crates.io which has locked dependencies compatible with rustc 1.85.0
# Installing from git causes cargo to resolve dependencies to latest versions,
# some of which (like home v0.5.12+) require rustc 1.88+
cargo install cross --version 0.2.5

if [ ! -z "$CI" ]; then

    echo "CARGO=cross" >> ${GITHUB_ENV}

    # See https://github.com/containerd/runwasi/pull/813#issuecomment-2619138618
    echo "CROSS_NO_WARNINGS=0" >> ${GITHUB_ENV}

    if [ ! -z "$1" ]; then
        echo "TARGET=$1" >> ${GITHUB_ENV}
    fi

fi
