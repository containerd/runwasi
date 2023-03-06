# Releasing a new crate version

This document describes the steps to release a new version of the crate.

## Overview

Releases are handled by the [release](.github/workflows/release.yml) GitHub actions workflow.
The workflow is triggered when a new tag is pushed to the repository following the pattern `<crate>/v<version>`.

In the future we may include a workflow for tagging the release but for now this is manual.

The release workflow will:
- Build the crate to be released (determined by the tag)
- Run the tests for that crate (and only that crate!)
- Build any associated release artifacts (e.g. the containerd-shim-wasmtime crate includes several binaries).
- Publish the crate to crates.io

The workflow utilizes a bot account (@containerd-runwasi-release-bot) to publish the crate to crates.io. The bot account is only used to get a limited-scope API token to publish the crate on crates.io. The token is stored as a secret in the repository and is only used by the release workflow.

## Steps

1. Open a PR to bump crate version in the Cargo.toml for that crate.
2. PR can be merged after 2 LGTMs
3. Tag the release with the format `<crate>/v<version>` (e.g. `containerd-shim-wasm/v0.2.0`)
4. Wait for the release workflow to complete
5. Manually verify the release on crates.io and on the GitHub releases page.
6. If this is the first time publishing this crate, see the [First release of a crate](#First-release-of-a-crate) section.

If step 1 and/or 2 is skipped, the release workflow will fail because the version in the Cargo.toml will not match the tag.

For step 5, some crates have binaries, such as the containerd-shim-wasmtime crate. These binaries are built as part of the release workflow and uploaded to the GitHub release page. You can download the binaries from the release page and verify that they work as expected.

## First release of a crate

If the crate has never been published to crates.io before then ownership of the crate will need to be configured.
The containerd/runwasi-committers team will need to be added as an owner of the crate.
The release workflow will automatically invite the person who triggered the worrkflow run to be an owner of the crate.
That person will need to accept the invite to be an owner of the crate and then manually add the containerd/runwasi-committers team as an owner of the crate.

```
cargo owner --add github:containerd:runwasi-committers <crate-name>
```

*This assumes you've already done `cargo login` with your personal account.
Alternatively, the cargo cli does support setting the token via an environment variable, `CARGO_REGISTRY_TOKEN` or as a CLI flag.*

Now all members of the containerd/runwasi-committers team will have access to manage the crate (after they have accepted the invite to the crate).