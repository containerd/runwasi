#!/bin/bash
cargo install cross --git https://github.com/cross-rs/cross

if [ ! -z "$CI" ]; then

    echo "CARGO=cross" >> ${GITHUB_ENV}

    # See https://github.com/containerd/runwasi/pull/813#issuecomment-2619138618
    echo "CROSS_NO_WARNINGS=0" >> ${GITHUB_ENV}

    if [ ! -z "$1" ]; then
        echo "TARGET=$1" >> ${GITHUB_ENV}
    fi

fi
