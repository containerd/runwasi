#!/bin/bash

# Cleanup contianerd cache after a failure in the containerd-client tests

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
