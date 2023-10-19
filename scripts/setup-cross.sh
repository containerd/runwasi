#!/bin/bash
cargo install cross --git https://github.com/cross-rs/cross
echo "CARGO=cross" >> ${GITHUB_ENV}

if [ ! -z "$1" ]; then
    echo "TARGET=$1" >> ${GITHUB_ENV}
fi