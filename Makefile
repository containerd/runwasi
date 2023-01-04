PREFIX ?= /usr/local
INSTALL ?= install
TEST_IMG_NAME ?= wasmtest:latest
export CONTAINERD_NAMESPACE ?= default

TARGET ?= debug
RELEASE_FLAG :=
ifeq ($(TARGET),release)
RELEASE_FLAG = --release
endif

DOCKER_BUILD ?= docker buildx build

KIND_CLUSTER_NAME ?= containerd-wasm

.PHONY: build
build:
	cargo build $(RELEASE_FLAG)

.PHONY: install
install:
	mkdir -p $(PREFIX)/bin
	$(INSTALL) target/$(TARGET)/containerd-shim-wasmtime-v1 $(PREFIX)/bin/
	$(INSTALL) target/$(TARGET)/containerd-shim-wasmtimed-v1 $(PREFIX)/bin/
	$(INSTALL) target/$(TARGET)/containerd-wasmtimed $(PREFIX)/bin/

.PHONY: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm:
	cd crates/wasi-demo-app && cargo build

.PHONY: target/wasm32-wasi/$(TARGET)/img.tar
target/wasm32-wasi/$(TARGET)/img.tar: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
	cd crates/wasi-demo-app && cargo build --features oci-v1-tar

load: target/wasm32-wasi/$(TARGET)/img.tar
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms $<

bin/kind: test/k8s/Dockerfile
	$(DOCKER_BUILD) --output=bin/ -f test/k8s/Dockerfile --target=kind .


test/k8s/_out/img: test/k8s/Dockerfile Cargo.toml Cargo.lock $(shell find . -type f -name '*.rs')
	mkdir -p $(@D) && $(DOCKER_BUILD) -f test/k8s/Dockerfile --iidfile=$(@) --load  .

.PHONY: test/k8s/cluster
test/k8s/cluster: target/wasm32-wasi/$(TARGET)/img.tar bin/kind test/k8s/_out/img bin/kind
	bin/kind create cluster --name $(KIND_CLUSTER_NAME) --image="$(shell cat test/k8s/_out/img)" && \
	bin/kind load image-archive --name $(KIND_CLUSTER_NAME) $(<)

.PHONY: test/k8s/deploy
test/k8s/deploy: test/k8s/cluster
	kubectl --context=kind-$(KIND_CLUSTER_NAME) apply -f test/k8s/deploy.yaml
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=90s

.PHONY: test/k8s/clean
test/k8s/clean:
	kind delete cluster --name $(KIND_CLUSTER_NAME)