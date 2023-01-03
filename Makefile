PREFIX ?= /usr/local
INSTALL ?= install
TEST_IMG_NAME ?= wasmtest:latest
export CONTAINERD_NAMESPACE ?= default

TARGET ?= debug
RELEASE_FLAG :=
ifeq ($(TARGET),release)
RELEASE_FLAG = --release
endif

.PHONY: build
build:
	cargo build $(RELEASE_FLAG)

.PHONY: install
install:
	$(INSTALL) target/$(TARGET)/containerd-shim-wasmtime-v1 $(PREFIX)/bin
	$(INSTALL) target/$(TARGET)/containerd-shim-wasmtimed-v1 $(PREFIX)/bin
	$(INSTALL) target/$(TARGET)/containerd-wasmtimed $(PREFIX)/bin

.PHONY: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm:
	cd crates/wasi-demo-app && cargo build

.PHONY: target/wasm32-wasi/$(TARGET)/img.tar
target/wasm32-wasi/$(TARGET)/img.tar: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
	cd crates/wasi-demo-app && cargo build --features oci-v1-tar

load: target/wasm32-wasi/$(TARGET)/img.tar
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms $<