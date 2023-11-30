# Contributors' Guide

This guide will help familiarize contributors to the `containerd/runwasi` repository.

## Prerequisite

First read the containerd project's [general guidelines around contribution](https://github.com/containerd/project/blob/main/CONTRIBUTING.md)
which apply to all containerd projects.

## Setting up your local environment

At a minimum, the Rust toolchain.  When using `rustup` the correct toolchain version is picked up from the [rust-toolchain.toml](./rust-toolchain.toml) so you don't need to worry about the version.

> ```
> curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
> ```

There are a few helper scripts that will install and configure required packages based on your OS. The end-to-end tests require a static binary, so installing [cross-rs](https://github.com/cross-rs/cross) is recommended.

If on ubuntu/debian you can use the following script. Refer to [youki's](https://github.com/containers/youki#dependencies) documentation for other systems. 

```
./scripts/setup-linux.sh
```

If on Windows use (use [git BASH](https://gitforwindows.org/) terminal which has shell emulator)

```
./scripts/setup-windows.sh
```

If you choose to always build with `cross`, you don't need any of these requirements above as they will be provided via the cross container.  This does require `docker` or `podman`. Refer to the [cross getting started page](https://github.com/cross-rs/cross/wiki/Getting-Started) for more details. 

Install cross:

```
scripts/setup-cross.sh
```

## Project structure

There are several projects in the repository:

- `containerd-shim-wasm` - main library that is used by runtimes to create shims. Most of the shared code lives here.
- `containerd-shim-wasm-test-modules` - library with wasm test modules used in testing framework
- `containerd-shim-<runtime>` - shims per runtime (wasmtime, wasmedge, wasmer, etc). These produce binaries that are the shims which containerd talks too.
- `oci-tar-builder` - library and executable that helps build OCI tar files.
- `wasi-demo-app` - wasm application that is used for demos and testing.

## Building the project

To build all the shims in this repository:

```
make build
```

To build a shim for specific runtime (wasmtime, wasmer, wasmedge, etc):

```
make build-<runtime>
```

By default the runtimes will build for your current OS and architecture.  If you want to build for a specific OS and architecture you can specify `TARGET`, where it matches a target in [Cross.toml](./Cross.toml). If your target doesn't match your host OS and architecture [Cross](https://github.com/cross-rs/cross) will be used. As an example will build a static binary:

```
TARGET=x86_64-unknown-linux-musl make build
```

## Running tests

### Unit tests

Unit tests are run via `make test`  or for a specific runtime `make test-<runtime>`. On linux the tests will run using `sudo`. This is configured in the `runner` field in [.cargo/config.toml](./.cargo/config.toml)

You should see some output like:
```terminal
make test
running 3 tests
test instance::tests::test_maybe_open_stdio ... ok
test instance::wasitest::test_delete_after_create ... ok
test instance::wasitest::test_wasi ... ok
```

### End to End tests

The e2e test run on [k3s](https://k3s.io/) and [kind](https://kind.sigs.k8s.io/).  A test image is built using [oci-tar-builder](./crates/oci-tar-builder/) and is loaded onto the clusters.  This test image is not pushed to an external registry so be sure to use the Makefile targets to build the image and load it on the cluster.

The deployment file in [test/k8s/Dockerfile](./test/k8s/Dockerfile) is run and verified that it deploys and runs successfully.  To execute the e2e tests in either kind or k3s:

```
make test/k8s-<runtime> # runs using kind
make test/k3s-<runtime>
```

OCI Wasm image requires containerd 1.7.7+ and can be tested with:

```
make test/k8s-oci-<runtime>
```

### Building the test image

This builds a [wasm application](crates/wasi-demo-app/) and packages it in an OCI format:

```
make test-image
```

## Code style

We use nightly [`rustfmt`](https://github.com/rust-lang/rustfmt) and [`clippy`](https://github.com/rust-lang/rust-clippy) for most linting rules. They are installed automatically with rustup. Use the `check` makefile target to run these tools and verify your code matches the expected style.

```
make check
```

You can auto-fix most styles using 

```
make fix
```

## Updating protobuf files

Ensure protoc and dev tools have been installed.  Run `make build` or to just generate the protos:

```
cargo build -p containerd-shim-wasm --no-default-features --features generate_bindings
```

## Adding new features

Most features will likely have most of the code in the `containerd-shim-wasm` project and a few runtime specific addtions to each runtime shim.  The general expectation is that the feature should be added to all runtimes. We will evaluate on a case by case basis exceptions, where runtimes may not be able to support a given feature or requires changes that make it hard to review.  In those cases we it may make sense to implement in follow up PR's for other runtimes.

A tip for developing a new feature is to implement it and test it with one runtime you are familiar with then add it to all the runtimes.  This makes it easier to test and iterate before making changes across all the runtimes.

## Getting in touch

There is a lot going on in the project. If you get lost, stop by our [slack and ask questions](./README.md#community)!