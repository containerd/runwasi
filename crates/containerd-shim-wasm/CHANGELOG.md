# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- OpenTelemetry Tracing Support ([#582](https://github.com/containerd/runwasi/pull/582), [#653](https://github.com/containerd/runwasi/pull/653))
- Enabled async, networking, and IP name lookup in Wasmtime ([#589](https://github.com/containerd/runwasi/pull/589))
- Re-enabled Benchmarking with cargo bench ([#612](https://github.com/containerd/runwasi/pull/612))
- Support for Generating New Artifact Types ([#631](https://github.com/containerd/runwasi/pull/631))
- End-to-End Tests for Wasm OCI Artifacts ([#661](https://github.com/containerd/runwasi/pull/661))

### Changed
- Made `tracing::instrument` macro optional ([#592](https://github.com/containerd/runwasi/pull/592))
- Upgraded youki Libcontainer to v0.3.3 ([#601](https://github.com/containerd/runwasi/pull/601))
- Configured dependabot to group patch updates ([#641](https://github.com/containerd/runwasi/pull/641))
- Improved `PathResolve` logic using RPITIT ([#654](https://github.com/containerd/runwasi/pull/654))
- Improved error messages in `Executor::exec` ([#655](https://github.com/containerd/runwasi/pull/655))
- Improved the getting started guide and Makefile for Windows ([#665](https://github.com/containerd/runwasi/pull/665))
- Modified behavior so that container environment variables are exclusively passed to WASI modules, enhancing security and isolation. ([#668](https://github.com/containerd/runwasi/pull/668))

### Fixed
- Corrected syntax errors in release scripts ([#603](https://github.com/containerd/runwasi/pull/603), [#604](https://github.com/containerd/runwasi/pull/604))
- Resolved CI failures in benchmark tests ([#669](https://github.com/containerd/runwasi/pull/669))
- [#680](https://github.com/containerd/runwasi/pull/680): fixed a failed test `test_envs_not_present` and renamed it to `test_envs_return_default_only`

### Deprecated
- Deprecated 'Shared' mode removed ([#671](https://github.com/containerd/runwasi/pull/671))

### Removed
- Removed dependency on `prost-types` ([#656](https://github.com/containerd/runwasi/pull/656))

[Unreleased]: <https://github.com/containerd/runwasi/compare/containerd-shim-wasm/v0.6.0..HEAD>