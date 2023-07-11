PREFIX ?= /usr/local
INSTALL ?= install
TEST_IMG_NAME ?= wasmtest:latest
RUNTIMES ?= wasmedge wasmtime
CONTAINERD_NAMESPACE ?= default
BINARIES ?= $(foreach runtime,$(RUNTIMES), \
	containerd-shim-$(runtime)-v1 \
	containerd-shim-$(runtime)d-v1 \
	containerd-$(runtime)d \
)

TARGET ?= debug
RELEASE_FLAG :=
ifeq ($(TARGET),release)
RELEASE_FLAG = --release
endif

DOCKER_BUILD ?= docker buildx build

KIND_CLUSTER_NAME ?= containerd-wasm

CARGO ?= cargo

BUILD_DIR := $(shell $(CARGO) run -q --bin build-dir-helper $(RELEASE_FLAG))

export

.PHONY: build
build:
	$(CARGO) build -p containerd-shim-wasm --features generate_bindings $(RELEASE_FLAG)
	$(CARGO) build $(RELEASE_FLAG)

.PHONY: check
check:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --all --all-targets -- -D warnings

.PHONY: fix
fix:
	$(CARGO) fmt --all
	$(CARGO) clippy --fix --all --all-targets -- -D warnings

.PHONY: test
test:
	RUST_LOG=trace $(CARGO) test --all --verbose -- --nocapture

.PHONY: install
install:
	install -Dt $(PREFIX)/bin/ $(addprefix ${BUILD_DIR}/,${BINARIES} libwasmedge.so.0)

.PHONY: uninstall
uninstall:
	rm -f $(addprefix $(PREFIX)/bin/,${BINARIES} libwasmedge.so.0)

.PHONY: test-image
test-image: target/wasm32-wasi/$(TARGET)/img.tar

.PHONY: test-image
test-image/clean:
	rm -rf target/wasm32-wasi/$(TARGET)/

.PHONY: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm:
	rustup target add wasm32-wasi
	cd crates/wasi-demo-app && $(CARGO) build $(RELEASE_FLAG)

.PHONY: target/wasm32-wasi/$(TARGET)/img.tar
target/wasm32-wasi/$(TARGET)/img.tar: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
	cd crates/wasi-demo-app && $(CARGO) build $(RELEASE_FLAG) --features oci-v1-tar

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

.PHONY: test/k8s
test/k8s: test/k8s/cluster
	kubectl --context=kind-$(KIND_CLUSTER_NAME) apply -f test/k8s/deploy.yaml
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=90s

.PHONY: test/k8s/clean
test/k8s/clean: bin/kind
	bin/kind delete cluster --name $(KIND_CLUSTER_NAME)

.PHONY: bin/wasmedge
bin/wasmedge: ${WASMEDGE_LIB_DIR}/libwasmedge.so;
${WASMEDGE_LIB_DIR}/libwasmedge.so:
	mkdir -p ${WASMEDGE_LIB_DIR}
	curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | \
		SHELL=none bash -s -- --version=0.13.1 --path ${CURDIR}/bin/wasmedge

.PHONY: bin/wasmedge/clean
bin/wasmedge/clean:
	rm -R ${CURDIR}/bin/wasmedge

.PHONY: bin/k3s
bin/k3s:
	mkdir -p bin && \
	curl -sfL https://get.k3s.io | INSTALL_K3S_BIN_DIR=$(PWD)/bin INSTALL_K3S_SYMLINK=skip INSTALL_K3S_NAME=runwasi sh -

.PHONY: bin/k3s/clean
bin/k3s/clean:
	bin/k3s-runwasi-uninstall.sh

.PHONY: test/k3s
test/k3s: target/wasm32-wasi/$(TARGET)/img.tar bin/k3s
	$(MAKE) build && \
	$(MAKE) install PREFIX=$(PWD) && \
	sudo cp /var/lib/rancher/k3s/agent/etc/containerd/config.toml /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '  runtime_type = "$(PWD)/bin/containerd-shim-wasmedge-v1"' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '  [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm.options]' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '    BinaryName = "$(PWD)/bin/wasmedge/bin/wasmedge"' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo "CONTAINERD_NAMESPACE='default'" | sudo tee /etc/systemd/system/k3s-runwasi.service.env && \
	echo "NO_PROXY=192.168.0.0/16" | sudo tee -a /etc/systemd/system/k3s-runwasi.service.env && \
	sudo systemctl daemon-reload && \
	sudo systemctl restart k3s-runwasi && \
	timeout 60 bash -c -- 'while true; do sudo bin/k3s ctr version && break; sleep 1; done' && \
	sudo bin/k3s ctr image import --all-platforms target/wasm32-wasi/$(TARGET)/img.tar && \
	sudo bin/k3s kubectl apply -f test/k8s/deploy.yaml
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=90s && \
	sudo bin/k3s kubectl get pods -o wide

.PHONY: test/k3s/clean
test/k3s/clean: bin/k3s/clean
	$(MAKE) uninstall PREFIX=$(PWD)
	$(CARGO) clean
