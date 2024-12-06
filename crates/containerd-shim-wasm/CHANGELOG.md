# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Added test for signal handling issue [#755](https://github.com/containerd/runwasi/issues/755) ([#756](https://github.com/containerd/runwasi/pull/756))

### Changed
- Reuse and synchronise access to `Container` object instead of reloading form disk ([#763](https://github.com/containerd/runwasi/pull/763))

### Removed
- Removed `containerd_shim_wasm::sandbox::instance_utils::get_instance_root` and `containerd_shim_wasm::sandbox::instance_utils::instance_exists` functions ([#763](https://github.com/containerd/runwasi/pull/763))

## [v0.8.0] — 2024-12-04

### Added
- Support for "wasi:http/proxy" world in Wasmtime shim ([#691](https://github.com/containerd/runwasi/pull/691), [#705](https://github.com/containerd/runwasi/pull/705))
- Re-export `containerd_shim::Config` ([#714](https://github.com/containerd/runwasi/pull/714))
- CI jobs for spell checking and running documentation tests ([#728](https://github.com/containerd/runwasi/pull/728))

### Changed
- Update the `opentelemetry` related dependencies to the latest version ([#712](https://github.com/containerd/runwasi/pull/712))
- Use `InstanceAllocationStrategy::Pooling` if possible in wasmtime-shim ([#751](https://github.com/containerd/runwasi/pull/751))

### Removed
- Removed special handling of the pause container, now it is treated as any other native container ([#724](https://github.com/containerd/runwasi/pull/724))

### Fixed
- Documentation tests ([#728](https://github.com/containerd/runwasi/pull/728))

## [v0.7.0] — 2024-10-7

### Added
- OpenTelemetry tracing support ([#582](https://github.com/containerd/runwasi/pull/582), [#653](https://github.com/containerd/runwasi/pull/653))
- Enabled async, networking, and IP name lookup in Wasmtime ([#589](https://github.com/containerd/runwasi/pull/589))
- Re-enabled benchmarking with cargo bench ([#612](https://github.com/containerd/runwasi/pull/612))
- Support for generating new artifact types ([#631](https://github.com/containerd/runwasi/pull/631))
- End-to-end tests for Wasm OCI artifacts ([#661](https://github.com/containerd/runwasi/pull/661))

### Changed
- Made `tracing::instrument` macro optional ([#592](https://github.com/containerd/runwasi/pull/592))
- Upgraded youki Libcontainer to v0.3.3 that reduce startup time by 1s ([#601](https://github.com/containerd/runwasi/pull/601))
- Configured dependabot to group patch updates ([#641](https://github.com/containerd/runwasi/pull/641))
- Improved `PathResolve` logic using RPITIT ([#654](https://github.com/containerd/runwasi/pull/654))
- Improved error messages in `Executor::exec` ([#655](https://github.com/containerd/runwasi/pull/655))
- Improved the getting started guide and Makefile for Windows ([#665](https://github.com/containerd/runwasi/pull/665))
- Modified behavior so that container environment variables are exclusively passed to WASI modules, enhancing security and isolation. ([#668](https://github.com/containerd/runwasi/pull/668))
- Updated the `containerd-shim` dependency to the latest version.

### Fixed
- Corrected syntax errors in release scripts ([#603](https://github.com/containerd/runwasi/pull/603), [#604](https://github.com/containerd/runwasi/pull/604))
- Resolved CI failures in benchmark tests ([#669](https://github.com/containerd/runwasi/pull/669))
- Fixed a failed test `test_envs_not_present` and renamed it to `test_envs_return_default_only` [#680](https://github.com/containerd/runwasi/pull/680)
- Fixed the setup environment by adding openssl dependency to the Dockerfile [#680](https://github.com/containerd/runwasi/pull/680)

### Deprecated
- Deprecated the 'Shared' mode ([#671](https://github.com/containerd/runwasi/pull/671))

### Removed
- Removed dependency on `prost-types` ([#656](https://github.com/containerd/runwasi/pull/656))
- Removed dependency on `native-tls` ([#683](https://github.com/containerd/runwasi/pull/683)), note that the `opentelemetry` feature still depends on `native-tls`.

[Unreleased]: <https://github.com/containerd/runwasi/compare/containerd-shim-wasm/v0.8.0..HEAD>
[v0.8.0]: <https://github.com/containerd/runwasi/compare/containerd-shim-wasm/v0.7.0...containerd-shim-wasm/v0.8.0>
[v0.7.0]: <https://github.com/containerd/runwasi/compare/containerd-shim-wasm/v0.6.0...containerd-shim-wasm/v0.7.0>
