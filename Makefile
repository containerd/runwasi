PREFIX ?= /usr/local
INSTALL ?= install
TEST_IMG_NAME ?= wasmtest:latest
RUNTIMES ?= wasmedge wasmtime
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
	$(foreach runtime,$(RUNTIMES), \
		$(INSTALL) target/$(TARGET)/containerd-shim-$(runtime)-v1 $(PREFIX)/bin/; \
		$(INSTALL) target/$(TARGET)/containerd-shim-$(runtime)d-v1 $(PREFIX)/bin/; \
		$(INSTALL) target/$(TARGET)/containerd-$(runtime)d $(PREFIX)/bin/; \
	)

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

.PHONY: bin/wasmedge
bin/wasmedge:
	curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash -s -- -p $(PWD)/bin/wasmedge

.PHONY: bin/wasmedge/clean
bin/wasmedge/clean:
	curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/uninstall.sh | bash -s -- -p $(PWD)/bin/wasmedge -q

.PHONY: bin/k3s
bin/k3s:
	mkdir -p bin
	curl -sfL https://get.k3s.io | INSTALL_K3S_BIN_DIR=$(PWD)/bin INSTALL_K3S_SYMLINK=skip INSTALL_K3S_NAME=runwasi sh - && \
	sudo cp /var/lib/rancher/k3s/agent/etc/containerd/config.toml /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '  runtime_type = "io.containerd.wasmedge.v1"' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '  [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm.options]' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '    BinaryName = "$(PWD)/bin/wasmedge/bin/wasmedge"' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo "NO_PROXY=192.168.0.0/16" | sudo tee -a /etc/systemd/system/k3s-runwasi.service.env && \
	sudo systemctl restart k3s-runwasi

.PHONY: bin/k3s/clean
bin/k3s/clean:
	bin/k3s-runwasi-uninstall.sh

.PHONY: test/k3s
test/k3s: bin/k3s bin/wasmedge target/wasm32-wasi/$(TARGET)/img.tar
	sudo bin/k3s ctr image import --all-platforms $<
	sudo bin/k3s kubectl apply -f test/k8s/deploy.yaml
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=90s
