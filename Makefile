PREFIX ?= /usr/local
INSTALL ?= install
CARGO ?= cargo
LN ?= ln -sf
TEST_IMG_NAME ?= wasmtest:latest
RUNTIMES ?= wasmedge wasmtime wasmer
CONTAINERD_NAMESPACE ?= default

TARGET ?= 
OPT_PROFILE ?= debug
RELEASE_FLAG :=
ifeq ($(OPT_PROFILE),release)
RELEASE_FLAG = --release
endif
ifneq ($(TARGET),)
TARGET_FLAG = --target=$(TARGET)
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

export

.PHONY: build build-common build-wasm build-%
build: build-wasm $(RUNTIMES:%=build-%);

build-common: build-wasm;
build-wasm:
	$(CARGO) build $(TARGET_FLAG) -p containerd-shim-wasm --no-default-features --features generate_bindings $(RELEASE_FLAG)
	$(CARGO) build $(TARGET_FLAG) -p containerd-shim-wasm $(FEATURES_wasm) $(RELEASE_FLAG)

build-%:
	$(CARGO) build $(TARGET_FLAG) -p containerd-shim-$* $(FEATURES_$*) $(RELEASE_FLAG)

.PHONY: check check-common check-wasm check-%
check: check-wasm $(RUNTIMES:%=check-%);

check-common: check-wasm;
check-wasm:
	$(CARGO) +nightly fmt $(TARGET_FLAG) -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules -- --check
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_wasm) -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules -- $(WARNINGS)

check-%:
	$(CARGO) +nightly fmt $(TARGET_FLAG) -p containerd-shim-$* -- --check
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_$*) -p containerd-shim-$* -- $(WARNINGS)

.PHONY: fix fix-common fix-wasm fix-%
fix: fix-wasm $(RUNTIMES:%=fix-%);

fix-common: fix-wasm;
fix-wasm:
	$(CARGO) +nightly fmt $(TARGET_FLAG) -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_wasm) --fix -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules -- $(WARNINGS)

fix-%:
	$(CARGO) +nightly fmt $(TARGET_FLAG) -p containerd-shim-$*
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_$*) --fix -p containerd-shim-$* -- $(WARNINGS)

.PHONY: test test-common test-wasm test-wasmedge test-%
test: test-wasm $(RUNTIMES:%=test-%);

test-common: test-wasm;
test-wasm:
	# oci-tar-builder and wasi-demo-app have no tests
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-wasm $(FEATURES_wasm) --verbose -- --nocapture --test-threads=1

test-wasmedge:
	# run tests in one thread to prevent paralellism
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-wasmedge $(FEATURES_wasmedge) --lib --verbose -- --nocapture --test-threads=1
ifneq ($(OS), Windows_NT)
	# run wasmedge test without the default `static` feature
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-wasmedge --no-default-features --features standalone --lib --verbose -- --nocapture --test-threads=1
endif

test-%:
	# run tests in one thread to prevent paralellism
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-$* $(FEATURES_$*) --lib --verbose -- --nocapture --test-threads=1

.PHONY: install install-%
install: $(RUNTIMES:%=install-%);

install-%: build-%
	mkdir -p $(PREFIX)/bin
	$(INSTALL) target/$(OPT_PROFILE)/containerd-shim-$*-v1 $(PREFIX)/bin/
	$(LN) ./containerd-shim-$*-v1 $(PREFIX)/bin/containerd-shim-$*d-v1
	$(LN) ./containerd-shim-$*-v1 $(PREFIX)/bin/containerd-$*d

.PHONY: dist dist-%
dist: $(RUNTIMES:%=dist-%);

dist-%:
	[ -f $(PWD)/dist/bin/containerd-shim-$*-v1 ] || $(MAKE) install-$* PREFIX="$(PWD)/dist" OPT_PROFILE="$(OPT_PROFILE)"

.PHONY: test-image
test-image: dist/img.tar

.PHONY: test-image
test-image/clean:
	rm -rf target/wasm32-wasi/$(OPT_PROFILE)/

.PHONY: target/wasm32-wasi/$(OPT_PROFILE)/wasi-demo-app.wasm
target/wasm32-wasi/$(OPT_PROFILE)/wasi-demo-app.wasm:
	rustup target add wasm32-wasi
	cd crates/wasi-demo-app && cargo build $(RELEASE_FLAG)

target/wasm32-wasi/$(OPT_PROFILE)/img.tar: target/wasm32-wasi/$(OPT_PROFILE)/wasi-demo-app.wasm
	cd crates/wasi-demo-app && cargo build $(RELEASE_FLAG) --features oci-v1-tar

.PHONY: dist/img.tar
dist/img.tar:
	@mkdir -p "dist/"
	[ -f $(PWD)/dist/img.tar ] || $(MAKE) target/wasm32-wasi/$(OPT_PROFILE)/img.tar
	[ -f $(PWD)/dist/img.tar ] || cp target/wasm32-wasi/$(OPT_PROFILE)/img.tar "$@"

load: dist/img.tar
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms $<

bin/kind: test/k8s/Dockerfile
	$(DOCKER_BUILD) --output=bin/ -f test/k8s/Dockerfile --target=kind .

test/k8s/_out/img-%: test/k8s/Dockerfile dist-%
	mkdir -p $(@D) && $(DOCKER_BUILD) -f test/k8s/Dockerfile --build-arg="RUNTIME=$*" --iidfile=$(@) --load  .

.PHONY: test/nginx
test/nginx:
	docker pull docker.io/nginx:latest
	mkdir -p $@/out && docker save -o $@/out/img.tar docker.io/nginx:latest

.PHONY: test/k8s/cluster-%
test/k8s/cluster-%: dist/img.tar bin/kind test/k8s/_out/img-%
	bin/kind create cluster --name $(KIND_CLUSTER_NAME) --image="$(shell cat test/k8s/_out/img-$*)" && \
	bin/kind load image-archive --name $(KIND_CLUSTER_NAME) $(<)

.PHONY: test/k8s-%
test/k8s-%: test/k8s/cluster-%
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

.PHONY: test/k3s-%
test/k3s-%: dist/img.tar bin/k3s dist-%
	sudo cp /var/lib/rancher/k3s/agent/etc/containerd/config.toml /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
	echo '[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.wasm]' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
	echo '  runtime_type = "$(PWD)/dist/bin/containerd-shim-$*-v1"' | sudo tee -a /var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl
	echo "CONTAINERD_NAMESPACE='$(CONTAINERD_NAMESPACE)'" | sudo tee /etc/systemd/system/k3s-runwasi.service.env
	echo "NO_PROXY=192.168.0.0/16" | sudo tee -a /etc/systemd/system/k3s-runwasi.service.env
	sudo systemctl daemon-reload
	sudo systemctl restart k3s-runwasi
	timeout 60 bash -c -- 'while ! sudo bin/k3s ctr version; do sleep 1; done'
	sudo bin/k3s ctr image import --all-platforms dist/img.tar
	timeout 60 bash -c -- 'while [ "$$(sudo bin/k3s kubectl get pods --all-namespaces --no-headers | wc -l)" == "0" ]; do sleep 1; done'
	timeout 60 bash -c -- 'while [ "$$(sudo bin/k3s kubectl get pods --all-namespaces --no-headers | grep -vE "Completed|Running" | wc -l)" != "0" ]; do sleep 1; done'
	sudo bin/k3s kubectl get pods --all-namespaces
	sudo bin/k3s kubectl apply -f test/k8s/deploy.yaml
	sudo bin/k3s kubectl get pods --all-namespaces
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=120s
	sudo bin/k3s kubectl get pods -o wide

.PHONY: test/k3s/clean
test/k3s/clean: bin/k3s/clean;
