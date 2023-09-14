PREFIX ?= /usr/local
INSTALL ?= install
LN ?= ln -sf
TEST_IMG_NAME ?= wasmtest:latest
RUNTIMES ?= wasmedge wasmtime wasmer
export CONTAINERD_NAMESPACE ?= default

TARGET ?= debug
RELEASE_FLAG :=
ifeq ($(TARGET),release)
RELEASE_FLAG = --release
endif

FEATURES_wasmedge = 
WARNINGS = -D warnings
ifeq ($(OS), Windows_NT)
# need to turn off static/standalone for wasm-edge
FEATURES_wasmedge = --no-default-features
# turn of warnings until windows is fully supported #49
WARNINGS = 
endif

DOCKER_BUILD ?= docker buildx build

KIND_CLUSTER_NAME ?= containerd-wasm

.PHONY: build build-common build-wasm build-%
build: build-wasm $(RUNTIMES:%=build-%);

build-common: build-wasm;
build-wasm:
	cargo build -p containerd-shim-wasm --no-default-features --features generate_bindings $(RELEASE_FLAG)
	cargo build -p containerd-shim-wasm $(FEATURES_wasm) $(RELEASE_FLAG)

build-%:
	cargo build -p containerd-shim-$* $(FEATURES_$*) $(RELEASE_FLAG)

.PHONY: check check-common check-wasm check-%
check: check-wasm $(RUNTIMES:%=check-%);

check-common: check-wasm;
check-wasm:
	cargo +nightly fmt -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test -- --check
	cargo clippy $(FEATURES_wasm) -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test -- $(WARNINGS)

check-%:
	cargo +nightly fmt -p containerd-shim-$* -- --check
	cargo clippy $(FEATURES_$*) -p containerd-shim-$* -- $(WARNINGS)

.PHONY: fix fix-common fix-wasm fix-%
fix: fix-wasm $(RUNTIMES:%=fix-%);

fix-common: fix-wasm;
fix-wasm:
	cargo +nightly fmt -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test
	cargo clippy $(FEATURES_wasm) --fix -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test -- $(WARNINGS)

fix-%:
	cargo +nightly fmt -p containerd-shim-$*
	cargo clippy $(FEATURES_$*) --fix -p containerd-shim-$* -- $(WARNINGS)

.PHONY: test test-common test-wasm test-wasmedge test-%
test: test-wasm $(RUNTIMES:%=test-%);

test-common: test-wasm;
test-wasm:
	# oci-tar-builder and wasi-demo-app have no tests
	RUST_LOG=trace cargo test --package containerd-shim-wasm $(FEATURES_wasm) --verbose -- --nocapture

test-wasmedge:
	# run tests in one thread to prevent paralellism
	RUST_LOG=trace cargo test --package containerd-shim-wasmedge $(FEATURES_wasmedge) --lib --verbose -- --nocapture --test-threads=1
ifneq ($(OS), Windows_NT)
	# run wasmedge test without the default `static` feature
	RUST_LOG=trace cargo test --package containerd-shim-wasmedge --no-default-features --features standalone --lib --verbose -- --nocapture --test-threads=1
endif

test-%:
	# run tests in one thread to prevent paralellism
	RUST_LOG=trace cargo test --package containerd-shim-$* $(FEATURES_$*) --lib --verbose -- --nocapture --test-threads=1

.PHONY: install install-%
install: $(RUNTIMES:%=install-%);

install-%:
	mkdir -p $(PREFIX)/bin
	$(INSTALL) target/$(TARGET)/containerd-shim-$*-v1 $(PREFIX)/bin/
	$(LN) ./containerd-shim-$*-v1 $(PREFIX)/bin/containerd-shim-$*d-v1
	$(LN) ./containerd-shim-$*-v1 $(PREFIX)/bin/containerd-$*d

# dist is not phony, so that if the folder exist we don't try to do work
dist:
	$(MAKE) $(RUNTIMES:%=dist-%) TARGET="$(TARGET)"

.PHONY: dist-%
dist-%: build-%
	$(MAKE) install-$* PREFIX="$(PWD)/dist" TARGET="$(TARGET)"

.PHONY: test-image
test-image: dist/img.tar

.PHONY: test-image
test-image/clean:
	rm -rf target/wasm32-wasi/$(TARGET)/

.PHONY: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm:
	rustup target add wasm32-wasi
	cd crates/wasi-demo-app && cargo build $(RELEASE_FLAG)

target/wasm32-wasi/$(TARGET)/img.tar: target/wasm32-wasi/$(TARGET)/wasi-demo-app.wasm
	cd crates/wasi-demo-app && cargo build $(RELEASE_FLAG) --features oci-v1-tar

dist/img.tar: target/wasm32-wasi/$(TARGET)/img.tar
	@mkdir -p "dist/"
	cp "$<" "$@"

load: dist/img.tar
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms $<

bin/kind: test/k8s/Dockerfile
	$(DOCKER_BUILD) --output=bin/ -f test/k8s/Dockerfile --target=kind .

test/k8s/_out/img: test/k8s/Dockerfile dist
	mkdir -p $(@D) && $(DOCKER_BUILD) -f test/k8s/Dockerfile --iidfile=$(@) --load  .

.PHONY: test/nginx
test/nginx:
	docker pull docker.io/nginx:latest
	mkdir -p $@/out && docker save -o $@/out/img.tar docker.io/nginx:latest

.PHONY: test/k8s/cluster
test/k8s/cluster: dist/img.tar bin/kind test/k8s/_out/img
	bin/kind create cluster --name $(KIND_CLUSTER_NAME) --image="$(shell cat test/k8s/_out/img)" && \
	bin/kind load image-archive --name $(KIND_CLUSTER_NAME) $(<)

.PHONY: test/k8s
test/k8s: test/k8s/cluster
	kubectl --context=kind-$(KIND_CLUSTER_NAME) apply -f test/k8s/deploy.yaml
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=90s

.PHONY: test/k8s/clean
test/k8s/clean: bin/kind
	bin/kind delete cluster --name $(KIND_CLUSTER_NAME)

.PHONY: bin/k3s
bin/k3s:
	mkdir -p bin && \
	curl -sfL https://get.k3s.io | INSTALL_K3S_BIN_DIR=$(PWD)/bin INSTALL_K3S_SYMLINK=skip INSTALL_K3S_NAME=runwasi sh -

.PHONY: bin/k3s/clean
bin/k3s/clean:
	bin/k3s-runwasi-uninstall.sh

.PHONY: test/k3s-wasmedge
test/k3s-wasmedge: dist/img.tar bin/k3s dist
	sudo cp /var/lib/rancher/k3s/agent/etc/containerd/config.toml /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '  runtime_type = "$(PWD)/dist/bin/containerd-shim-wasmedge-v1"' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo "CONTAINERD_NAMESPACE='default'" | sudo tee /etc/systemd/system/k3s-runwasi.service.env && \
	echo "NO_PROXY=192.168.0.0/16" | sudo tee -a /etc/systemd/system/k3s-runwasi.service.env && \
	sudo systemctl daemon-reload && \
	sudo systemctl restart k3s-runwasi && \
	timeout 60 bash -c -- 'while true; do sudo bin/k3s ctr version && break; sleep 1; done' && \
	sudo bin/k3s ctr image import --all-platforms dist/img.tar && \
	sudo bin/k3s kubectl apply -f test/k8s/deploy.yaml
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=120s && \
	sudo bin/k3s kubectl get pods -o wide

.PHONY: test/k3s-wasmer
test/k3s-wasmer: dist/img.tar bin/k3s dist
	sudo cp /var/lib/rancher/k3s/agent/etc/containerd/config.toml /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo '  runtime_type = "$(PWD)/dist/bin/containerd-shim-wasmer-v1"' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl && \
	echo "CONTAINERD_NAMESPACE='default'" | sudo tee /etc/systemd/system/k3s-runwasi.service.env && \
	echo "NO_PROXY=192.168.0.0/16" | sudo tee -a /etc/systemd/system/k3s-runwasi.service.env && \
	sudo systemctl daemon-reload && \
	sudo systemctl restart k3s-runwasi && \
	timeout 60 bash -c -- 'while true; do sudo bin/k3s ctr version && break; sleep 1; done' && \
	sudo bin/k3s ctr image import --all-platforms dist/img.tar && \
	sudo bin/k3s kubectl apply -f test/k8s/deploy.yaml
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=120s && \
	sudo bin/k3s kubectl get pods -o wide

.PHONY: test/k3s/clean
test/k3s/clean: bin/k3s/clean;
