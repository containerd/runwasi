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

# TODO: build this manually instead of requiring buildx
test/out/img.tar: test/image/Dockerfile test/image/src/main.rs test/image/Cargo.toml test/image/Cargo.lock
	mkdir -p $(@D)
	docker buildx rm wasmbuilder || true
	docker buildx create --name wasmbuilder --use
	docker buildx build --platform=wasi/wasm -o type=docker,dest=$@ -t $(TEST_IMG_NAME) ./test/image

load: test/out/img.tar
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import $<

clean:
	rm -rf target/$(TARGET)
	rm test/out/img.tar
