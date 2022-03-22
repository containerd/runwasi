PREFIX ?= /usr/local
INSTALL ?= install
TEST_IMG_NAME ?= wasmtest:latest
export CONTAINERD_NAMESPACE

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

# TODO: build this manually instead of requiring buildx
test/out/img.tar: test/image/Dockerfile test/image/wasm.go
	mkdir -p $(@D)
	docker buildx build --platform=wasi/wasm -o type=docker,dest=$@ -t $(TEST_IMG_NAME) ./test/image

load: test/out/img.tar
	sudo ctr image import $<