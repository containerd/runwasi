#!/bin/bash

# Cleanup contianerd content cache and leases after a failure in the containerd-client tests

# The containerd-client tests interact with the real containerd in the computer, creating leases and caching content in different namespaces.
# If a containerd-client tests is interrupted (by either a test failure during development, or by the user with ctrl-c), containerd is
# left polluted with a bunch of leases and cached content.
# This can lead to subsequent test runs failing due to the pre-existing leases being present.

# This cleans-up any remaining leases and cached content that the test might have left behind

function cleanup() {
    IDS="$(ctr --namespace $1 $2 ls | tail -n +2 | awk '{print $1}')"
    if [ "$IDS" != "" ]; then
        echo $IDS
        ctr --namespace $1 $2 rm $IDS
    fi
}

cleanup runwasi-test leases
cleanup runwasi-test content
cleanup test-ns leases
cleanup test-ns content
cleanup test leases
cleanup test content
