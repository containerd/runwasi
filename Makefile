PREFIX ?= /usr/local
INSTALL ?= install
CARGO ?= cargo
TEST_IMG_NAME ?= wasmtest:latest
RUNTIMES ?= wasmedge wasmtime wasmer wamr
CONTAINERD_NAMESPACE ?= default
RUSTC ?= rustc

SHELL=/bin/bash -o pipefail

# We have a bit of fancy logic here to determine the target 
# since we support building for gnu and musl
# TARGET must eventually match one of the values in the cross.toml
HOST_TARGET = $(shell $(RUSTC) --version -v | sed -En 's/host: (.*)/\1/p')

# if TARGET is not set and we are using cross
# default to musl to facilitate easier use shim on other distros because of the static build
# otherwise use the host target
ifeq ($(TARGET),)
ifeq ($(CARGO),cross)
override TARGET = $(shell uname -m)-unknown-linux-musl
else
override TARGET = $(HOST_TARGET)
endif
endif

# always use cross when the target is not the host target
ifneq ($(TARGET),$(HOST_TARGET))
override CARGO = cross
endif

ifeq ($(CARGO),cross)
override TARGET_DIR := $(or $(TARGET_DIR),./target/build/$(TARGET)/)
# When using `cross` we need to run the tests outside the `cross` container.
# We stop `cargo test` from running the tests with the `--no-run` flag.
# We then need to run the generate test binary manually.
# For that we use `--message-format=json` and `jq` to find the name of the binary, `xargs` and execute it.
TEST_ARGS_SEP= --no-run --color=always --message-format=json | \
	jq -R '. as $$line | try (fromjson | .executable | strings) catch ($$line+"\n" | stderr | empty)' -r | \
	sed -E 's|^/target|$(TARGET_DIR)|' | \
	xargs -I_ ./scripts/test-runner.sh ./_
else
override TARGET_DIR := $(or $(TARGET_DIR),./target/)
TEST_ARGS_SEP= --
endif
TARGET_FLAG = --target=$(TARGET) --target-dir=$(TARGET_DIR)

OPT_PROFILE ?= debug
RELEASE_FLAG :=
ifeq ($(OPT_PROFILE),release)
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

# turn opentelemetry feature on
FEATURES_wasm ?= -F opentelemetry

DOCKER_BUILD ?= docker buildx build

KIND_CLUSTER_NAME ?= containerd-wasm

export

.PHONY: build build-common build-wasm build-%
build: build-wasm $(RUNTIMES:%=build-%);

build-common: build-wasm;
build-wasm:
	$(CARGO) build $(TARGET_FLAG) -p containerd-shim-wasm $(FEATURES_wasm) $(RELEASE_FLAG)

build-%:
	$(CARGO) build $(TARGET_FLAG) -p containerd-shim-$* $(FEATURES_$*) $(RELEASE_FLAG)

build-oci-tar-builder:
	$(CARGO) build $(TARGET_FLAG) -p oci-tar-builder $(FEATURES_$*) $(RELEASE_FLAG)

.PHONY: check check-common check-wasm check-%
check: check-wasm $(RUNTIMES:%=check-%);

check-common: check-wasm;
check-wasm:
	# clear CARGO envvar as it otherwise interferes with rustfmt
	CARGO= $(CARGO) +nightly fmt -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules -- --check
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_wasm) -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules -- $(WARNINGS)

check-%:
	# clear CARGO envvar as it otherwise interferes with rustfmt
	CARGO= $(CARGO) +nightly fmt -p containerd-shim-$* -- --check
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_$*) -p containerd-shim-$* -- $(WARNINGS)

.PHONY: fix fix-common fix-wasm fix-%
fix: fix-wasm $(RUNTIMES:%=fix-%);

fix-common: fix-wasm;
fix-wasm:
	# clear CARGO envvar as it otherwise interferes with rustfmt
	CARGO= $(CARGO) +nightly fmt -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_wasm) --fix -p oci-tar-builder -p wasi-demo-app -p containerd-shim-wasm -p containerd-shim-wasm-test-modules -- $(WARNINGS)

fix-%:
	# clear CARGO envvar as it otherwise interferes with rustfmt
	CARGO= $(CARGO) +nightly fmt -p containerd-shim-$*
	$(CARGO) clippy $(TARGET_FLAG) $(FEATURES_$*) --fix -p containerd-shim-$* -- $(WARNINGS)

.PHONY: test test-common test-wasm test-wasmedge test-%
test: test-wasm $(RUNTIMES:%=test-%);

test-common: test-wasm;
test-wasm:
	# oci-tar-builder and wasi-demo-app have no tests
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-wasm $(FEATURES_wasm) --verbose $(TEST_ARGS_SEP) --nocapture --test-threads=1

test-wasmedge:
	# run tests in one thread to prevent parallelism
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-wasmedge $(FEATURES_wasmedge) --lib --verbose $(TEST_ARGS_SEP) --nocapture --test-threads=1
ifneq ($(OS), Windows_NT)
ifneq ($(patsubst %-musl,,xx_$(TARGET)),)
	# run wasmedge test without the default `static` feature
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-wasmedge --no-default-features --features standalone --lib --verbose $(TEST_ARGS_SEP) --nocapture --test-threads=1
endif
endif

test-%:
	# run tests in one thread to prevent parallelism
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package containerd-shim-$* $(FEATURES_$*) --lib --verbose $(TEST_ARGS_SEP) --nocapture --test-threads=1

test-doc:
	RUST_LOG=trace $(CARGO) test --doc -- --test-threads=1

generate-doc:
	RUST_LOG=trace $(CARGO) doc --workspace --all-features --no-deps --document-private-items --exclude wasi-demo-app

test-oci-tar-builder:
	RUST_LOG=trace $(CARGO) test $(TARGET_FLAG) --package oci-tar-builder $(FEATURES_$*) --verbose $(TEST_ARGS_SEP) --nocapture --test-threads=1

.PHONY: install install-%
install: $(RUNTIMES:%=install-%);

install-%:
	mkdir -p $(PREFIX)/bin
	$(INSTALL) $(TARGET_DIR)$(if $(TARGET),$(TARGET)/,)$(OPT_PROFILE)/containerd-shim-$*-v1 $(PREFIX)/bin/

install-oci-tar-builder:
	mkdir -p $(PREFIX)/bin
	$(INSTALL) $(TARGET_DIR)$(if $(TARGET),$(TARGET)/,)$(OPT_PROFILE)/oci-tar-builder $(PREFIX)/bin/

.PHONY: dist dist-%
dist: $(RUNTIMES:%=dist-%);

dist-%:
	[ -f $(PWD)/dist/bin/containerd-shim-$*-v1 ] || $(MAKE) install-$* CARGO=$(CARGO) PREFIX="$(PWD)/dist" OPT_PROFILE="$(OPT_PROFILE)"

.PHONY: dist/clean
dist/clean:
	rm -rf dist

.PHONY: install/all
install/all: test-image/clean install test-image load

.PHONY: install/oci/all
install/oci/all: test-image/oci/clean install test-image/oci load/oci

.PHONY: test-image
test-image: dist/img.tar

.PHONY: test-image/oci
test-image/oci: dist/img-oci.tar dist/img-oci-artifact.tar

.PHONY: test-image/http
test-image/http: dist/http-img-oci.tar

.PHONY: test-image/clean
test-image/clean:
	rm -rf target/wasm32-wasip1/$(OPT_PROFILE)/

.PHONY: test-image/oci/clean
test-image/oci/clean:
	rm -rf target/wasm32-wasip1/$(OPT_PROFILE)/img-oci.tar
	rm -rf target/wasm32-wasip1/$(OPT_PROFILE)/img-oci-artifact.tar

.PHONY: demo-app
demo-app: target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm

.PHONY: target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm
target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm:
	rustup target add wasm32-wasip1
	cd crates/wasi-demo-app && cargo build $(RELEASE_FLAG)

target/wasm32-wasip1/$(OPT_PROFILE)/img.tar: target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm
	cd crates/wasi-demo-app && cargo build $(RELEASE_FLAG) --features oci-v1-tar

.PHONY: dist/img.tar
dist/img.tar:
	@mkdir -p "dist/"
	[ -f $(PWD)/dist/img.tar ] || $(MAKE) target/wasm32-wasip1/$(OPT_PROFILE)/img.tar
	[ -f $(PWD)/dist/img.tar ] || cp target/wasm32-wasip1/$(OPT_PROFILE)/img.tar "$@"

dist/img-oci.tar: target/wasm32-wasip1/$(OPT_PROFILE)/img-oci.tar 
	@mkdir -p "dist/"
	cp "$<" "$@"

dist/img-oci-artifact.tar: target/wasm32-wasip1/$(OPT_PROFILE)/img-oci-artifact.tar
	@mkdir -p "dist/"
	cp "$<" "$@"

load: dist/img.tar
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms $<

CTR_VERSION := $(shell sudo ctr version | sed -n -e '/Version/ {s/.*: *//p;q;}')
load/oci: dist/img-oci.tar dist/img-oci-artifact.tar
	@echo $(CTR_VERSION)\\nv1.7.7 | sort -crV || @echo $(CTR_VERSION)\\nv1.6.25 | sort -crV || (echo "containerd version must be 1.7.7+ or 1.6.25+ was $(CTR_VERSION)" && exit 1)
	@echo using containerd $(CTR_VERSION)
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms $<
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms dist/img-oci-artifact.tar

.PHONY: load/http
load/http: dist/http-img-oci.tar
	sudo ctr -n $(CONTAINERD_NAMESPACE) image import --all-platforms $<

target/wasm32-wasip1/$(OPT_PROFILE)/img-oci.tar: target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm
	mkdir -p ${CURDIR}/bin/$(OPT_PROFILE)/
	cargo run --bin oci-tar-builder -- --name wasi-demo-oci --repo ghcr.io/containerd/runwasi --tag latest --module ./target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm -o target/wasm32-wasip1/$(OPT_PROFILE)/img-oci.tar

.PHONY:
target/wasm32-wasip1/$(OPT_PROFILE)/img-oci-artifact.tar: target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm
	mkdir -p ${CURDIR}/bin/$(OPT_PROFILE)/
	cargo run --bin oci-tar-builder -- --name wasi-demo-oci-artifact --as-artifact --repo ghcr.io/containerd/runwasi --tag latest --module ./target/wasm32-wasip1/$(OPT_PROFILE)/wasi-demo-app.wasm -o target/wasm32-wasip1/$(OPT_PROFILE)/img-oci-artifact.tar

.PHONY:
dist/http-img-oci.tar: crates/containerd-shim-wasm-test-modules/src/modules/hello_wasi_http.wasm
	@mkdir -p "dist/"
	cargo run --bin oci-tar-builder -- \
		--name wasi-http \
		--repo ghcr.io/containerd/runwasi \
		--tag latest \
		--module $< \
		-o $@

bin/kind: test/k8s/Dockerfile
	$(DOCKER_BUILD) --output=bin/ -f test/k8s/Dockerfile --target=kind .

# Use a static build of the shims for better compatibility.
# Using cross with no target defaults to <arch>-unknown-linux-musl, which creates a static binary.
test/k8s/_out/img-%: override CARGO=cross TARGET= TARGET_DIR=
test/k8s/_out/img-%: test/k8s/Dockerfile dist-%
	mkdir -p $(@D) && $(DOCKER_BUILD) -f test/k8s/Dockerfile --build-arg="RUNTIME=$*" --iidfile=$(@) --load .

test/k8s/_out/img-oci-%: test/k8s/Dockerfile.oci dist-%
	mkdir -p $(@D) && $(DOCKER_BUILD) -f test/k8s/Dockerfile.oci --build-arg="RUNTIME=$*" --iidfile=$(@) --load .

.PHONY: test/nginx
test/nginx:
	docker pull docker.io/nginx:latest
	mkdir -p $@/out && docker save -o $@/out/img.tar docker.io/nginx:latest

.PHONY: test/k8s/cluster-%
test/k8s/cluster-%: dist/img.tar bin/kind test/k8s/_out/img-%
	bin/kind create cluster --name $(KIND_CLUSTER_NAME) --image="$(shell cat test/k8s/_out/img-$*)" && \
	bin/kind load image-archive --name $(KIND_CLUSTER_NAME) $(<)


.PHONY: test/k8s/deploy-workload-%
test/k8s/deploy-workload-%: test/k8s/clean test/k8s/cluster-% 
	kubectl --context=kind-$(KIND_CLUSTER_NAME) apply -f test/k8s/deploy.yaml
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=300s
	# verify that we are still running after some time	
	sleep 5s
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=5s

.PHONY: test/k8s/deploy-workload-oci-%
test/k8s/deploy-workload-oci-%: test/k8s/clean test/k8s/cluster-% dist/img-oci.tar dist/img-oci-artifact.tar test/k8s/cluster-%
	bin/kind load image-archive --name $(KIND_CLUSTER_NAME) dist/img-oci.tar
	bin/kind load image-archive --name $(KIND_CLUSTER_NAME) dist/img-oci-artifact.tar
	kubectl --context=kind-$(KIND_CLUSTER_NAME) apply -f test/k8s/deploy.oci.yaml
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=300s
	# verify that we are still running after some time
	sleep 5s
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=5s
	@if [ "$*" = "wasmtime" ]; then \
		set -e; \
		echo "checking for pre-compiled labels and ensuring can scale after pre-compile"; \
		docker exec $(KIND_CLUSTER_NAME)-control-plane ctr -n k8s.io content ls | grep "runwasi.io/precompiled"; \
		kubectl --context=kind-$(KIND_CLUSTER_NAME) scale deployment wasi-demo --replicas=4; \
		kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for condition=Available=True --timeout=5s; \
	fi

.PHONY: test/k8s-%
test/k8s-%: test/k8s/deploy-workload-%
	# verify that we are able to delete the deployment
	kubectl --context=kind-$(KIND_CLUSTER_NAME) delete -f test/k8s/deploy.yaml
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for delete --timeout=60s

.PHONY: test/k8s-oci-%
test/k8s-oci-%: test/k8s/deploy-workload-oci-%
	# verify that we are able to delete the deployment
	kubectl --context=kind-$(KIND_CLUSTER_NAME) delete -f test/k8s/deploy.oci.yaml
	kubectl --context=kind-$(KIND_CLUSTER_NAME) wait deployment wasi-demo --for delete --timeout=60s

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
	sudo bash -c -- 'while ! timeout 40 test/k3s/bootstrap.sh "$*" dist/img.tar; do $(MAKE) bin/k3s/clean bin/k3s; done'
	sudo bin/k3s kubectl get pods --all-namespaces
	sudo bin/k3s kubectl apply -f test/k8s/deploy.yaml
	sudo bin/k3s kubectl get pods --all-namespaces
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=300s
	# verify that we are still running after some time	
	sleep 5s
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=5s
	sudo bin/k3s kubectl get pods -o wide
	sudo bin/k3s kubectl delete -f test/k8s/deploy.yaml
	sudo bin/k3s kubectl wait deployment wasi-demo --for delete --timeout=60s

.PHONY: test/k3s-oci-%
test/k3s-oci-%: dist/img-oci.tar bin/k3s dist-%
	sudo bash -c -- 'while ! timeout 40 test/k3s/bootstrap.sh "$*" dist/img-oci.tar; do $(MAKE) bin/k3s/clean bin/k3s; done'
	sudo bin/k3s kubectl get pods --all-namespaces
	sudo bin/k3s kubectl apply -f test/k8s/deploy.oci.yaml
	sudo bin/k3s kubectl get pods --all-namespaces
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=300s
	# verify that we are still running after some time	
	sleep 5s
	sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=5s
	sudo bin/k3s kubectl get pods -o wide
	@if [ "$*" = "wasmtime" ]; then \
		set -e; \
		echo "checking for pre-compiled labels and ensuring can scale"; \
		sudo bin/k3s ctr -n k8s.io content ls | grep "runwasi.io/precompiled"; \
		sudo bin/k3s kubectl scale deployment wasi-demo --replicas=4; \
		sudo bin/k3s kubectl wait deployment wasi-demo --for condition=Available=True --timeout=5s; \
	fi
	sudo bin/k3s kubectl delete -f test/k8s/deploy.oci.yaml
	sudo bin/k3s kubectl wait deployment wasi-demo --for delete --timeout=60s

.PHONY: test/k3s/clean
test/k3s/clean: bin/k3s/clean;

.PHONY: bench
bench:
	$(CARGO) bench -p containerd-shim-benchmarks

.PHONY: clean
clean:
	-rm -rf dist
	-rm -rf bin
	-rm -rf test/k8s/_out
	-$(MAKE) test-image/clean
	-$(MAKE) test/k8s/clean
	-$(MAKE) test/k3s/clean
