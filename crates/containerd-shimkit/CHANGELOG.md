# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [v0.1.1] - 2025-03-27

### Added

- Added `containerd_shimkit::set_logger_kv` to allow for setting logger key values. This is useful for passing the container ID and pod ID to the shim.

## [v0.1.0] - 2025-03-26

### Added
- Moved lower level (non wasi/wasm specific) APIs from `containerd-shim-wasm` to `containerd-shimkit`. ([#930](https://github.com/containerd/runwasi/pull/930))

[Unreleased]: <https://github.com/containerd/runwasi/compare/containerd-shimkit/v0.1.1..HEAD>
[v0.1.1]: <https://github.com/containerd/runwasi/compare/containerd-shimkit/v0.1.0...containerd-shimkit/v0.1.1>
