PREFIX ?= /usr/local
INSTALL ?= install
GO ?= go
TEST_IMG_NAME ?= wasmtest:latest

.PHONY: bin
build:
	$(GO) build -o bin/containerd-shim-wasmtime-v1 ./cmd/containerd-shim-wasmtime-v1

.PHONY: install
install:
	$(INSTALL) bin/* $(PREFIX)/bin

# TODO: build this manually instead of requiring buildx
.PHONY: test/out/img.tar
test/out/img.tar: test/image/Dockerfile test/image/wasm.go
	mkdir -p $(@D)
	docker buildx build --platform=wasi/wasm -o type=docker,dest=$@ -t $(TEST_IMG_NAME) ./test/image

load: test/out/img.tar
	sudo ctr image import $<