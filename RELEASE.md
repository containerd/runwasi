# Releasing a new crate version

This document describes the steps to release a new version of the crate.

## Overview

To create a new release, either run the release.yml workflow as a workload_dispatch trigger through the GitHub UI, or via the following command substituting the proper values for crate and version.
```bash
gh workflow run release.yml -f dry_run=true -f crate=containerd-shim-wasm -f version=0.4.0
```

### Input Values for Release.yml
- `crate:` [string] the name of the crate within the runwasi project. It should be a directory under `./crates`.
- `version:` [string] the version of the crate to stamp, tag, and release (e.g., 1.0.0, 0.6.0-rc1)
- `dry_run:` [boolean] a flag that causes the workflow to run all step except ones that would tag or push artifacts.

### Crate Release Sequence

Must release the creates in this order due to dependencies:
1. `containerd-shim-wasm`
2. All runtime-related crates.

## Release Steps

1. Open a PR to bump crate versions and dependency versions in `Cargo.toml` for that crate, and change the "Unreleased" section in the `CHANGELOG.md` to the new version.
2. PR can be merged after 2 LGTMs
3. Run the release workflow for the dependent crate. (e.g. `containerd-shim-wasm/v0.2.0` where `crate=containerd-shim-wasm` and `version=0.2.0`)
4. Wait for the release workflow to complete
5. Manually verify the release on crates.io and on the GitHub releases page (See [Verify signing](#Verify-signing) section for more details on verifying the release on GitHub releases page.)
6. If this is the first time publishing this crate, see the [First release of a crate](#First-release-of-a-crate) section.

> Note: If step 1 and/or 2 is skipped, the release workflow will fail because the version in the Cargo.toml will not match the tag.
>
> For step 5, some crates have binaries, such as the containerd-shim-wasmtime crate. These binaries are built as part of the release workflow and uploaded to the GitHub release page. You can download the binaries from the release page and verify that they work as expected.

## Local Development vs. Release
Locally, crates reference local paths. During release, they target published versions.
Use both `path` and `version` fields in the workspace `Cargo.toml`:

e.g.

```toml
containerd-shim-wasm = { path = "crates/containerd-shim-wasm", version = "0.4.0" }
```

## Verify signing

The release pipeline uses `cosign` to sign the release blobs, if any. It uses Github's OIDC token to authenticate with Sigstore to prove identity and outputs a `.bundle` file, which contains a signature and a key. This file can be verified using `cosign verify-blob` command, providing the workflow tag and Github as the issuer. The full command looks like this (e.g. wasmtime shim):

```sh
cosign verify-blob --bundle containerd-shim-wasmtime-v1.bundle \
--certificate-identity https://github.com/containerd/runwasi/.github/workflows/release.yml@refs/tags/containerd-shim-wasmtime/<tag> \ 
--certificate-oidc-issuer https://token.actions.githubusercontent.com \
containerd-shim-wasmtime-v1
```

In the Github release page, please provide the above command in the instructions for the consumer to verify the release.

## First time release of a crate

If the crate has never been published to crates.io before then ownership of the crate will need to be configured.
The containerd/runwasi-committers team will need to be added as an owner of the crate.
The release workflow will automatically invite the person who triggered the workflow run to be an owner of the crate.
That person will need to accept the invite to be an owner of the crate and then manually add the containerd/runwasi-committers team as an owner of the crate.

```
cargo owner --add github:containerd:runwasi-committers <crate-name>
```

*This assumes you've already done `cargo login` with your personal account.
Alternatively, the cargo cli does support setting the token via an environment variable, `CARGO_REGISTRY_TOKEN` or as a CLI flag.*

Now all members of the containerd/runwasi-committers team will have access to manage the crate (after they have accepted the invite to the crate).

## Release workflow summary

The workflow performs the following steps:
- Verifies inputs
- Verifies ability to push crates
- Updates the version of the crate to the version specified in the workflow input
- Build the crate to be released (determined by the tag), including any artifacts (e.g., associated binaries)
- Run the tests for that crate (and only that crate!)
- Publishes to the crates.io
- Tags the repository for the release
- Creates a GitHub release for that crate (attaching any artifacts)

The workflow utilizes a bot account (@containerd-runwasi-release-bot) to publish the crate to crates.io. The bot account is only used to get a limited-scope API token to publish the crate on crates.io. The token is stored as a secret in the repository and is only used by the release workflow.
