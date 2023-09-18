# Runs a recipe for a step in the Dockerfile.
# Usage:
#   dockerfile-utils <step>
#
# where step is one of `install_host`, `install_target` or `build_setup`.
#  * install_host:   installs build tools and other host environment dependencies (things that run in the host).
#  * install_target: installs build dependencies like libraries (things that will run in the target).
#  * build_setup:    setup to customize the build process (mainly setting env-vars)
#
# This script choosses the correct recipe based on the linux distribution of the base image.
# Supported distributions are `debian` and `alpine`.

dockerfile_utils_alpine() {
    # recipes for alpine

    install_host() {
        apk add g++ bash clang pkgconf git protoc jq
        apk add --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main rust-bindgen
    }

    install_target() {
        xx-apk add \
            gcc g++ musl-dev zlib-dev zlib-static \
            ncurses-dev ncurses-static libffi-dev \
            libseccomp-dev libseccomp-static
    }

    setup_build() {
        export WASMEDGE_DEP_STDCXX_LINK_TYPE="static"
        export WASMEDGE_DEP_STDCXX_LIB_PATH="$(xx-info sysroot)usr/lib"
        export WASMEDGE_RUST_BINDGEN_PATH="$(which bindgen)"
        export LIBSECCOMP_LINK_TYPE="static"
        export LIBSECCOMP_LIB_PATH="$(xx-info sysroot)usr/lib"
        export RUSTFLAGS="-Cstrip=symbols -Clink-arg=-lgcc"
    }

}

dockerfile_utils_debian() {
    # recipes for debian

    install_host() {
        apt-get update -y
        apt-get install --no-install-recommends -y clang pkg-config dpkg-dev git jq
    }

    install_target() {
        xx-apt-get install -y \
            gcc g++ libc++6-dev zlib1g \
            libsystemd-dev libdbus-1-dev libseccomp-dev
    }

    setup_build() {
        export RUSTFLAGS="-Cstrip=symbols"
    }

}

dockerfile_utils_$(xx-info vendor)
$1
