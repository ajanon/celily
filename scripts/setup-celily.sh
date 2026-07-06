#!/bin/bash -eu
#
# Install Rust and build celily for the current system.

declare -r SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "${SCRIPT_DIR}/utils.sh"

install_build_deps() {
    case "${DISTRO_ID}" in
        arch)
            package_install base-devel rustup
            ;;
        debian | kali)
            package_install rustup
            ;;
        fedora)
            package_install rustup gcc
            rustup-init -qy --profile=minimal --default-toolchain=nightly
            . "${HOME}/.cargo/env"
            ;;
        opensuse*)
            package_install rustup
            ;;
        ubuntu)
            snap_install rustup --classic
            package_install build-essential
            ;;
        void)
            package_install rustup
            ;;
        *)
            printf 'Unsupported distro: %s\n.' "${DISTRO_ID}" >&2
            return 1
            ;;
    esac
    if [[ "${DISTRO_ID}" != fedora ]]; then
        rustup toolchain install --profile=minimal nightly
        rustup default nightly
    fi
}

install_celily_deps() {
    case "${DISTRO_ID}" in
        arch | kali | void)
            package_install mitmproxy
            ;;
        opensuse-tumbleweed)
            package_install python3-mitmproxy
            ;;
        debian | fedora | ubuntu)
            package_install pipx
            pipx install -qq mitmproxy
            ln -s /root/.local/bin/mitmdump /usr/local/bin/mitmdump
            ;;
        opensuse-leap)
            package_install python3-pipx
            pipx install -qq mitmproxy
            ln -s /root/.local/bin/mitmdump /usr/local/bin/mitmdump
            ;;
        *)
            printf 'Unsupported distro: %s\n.' "${DISTRO_ID}" >&2
            return 1
            ;;
    esac
}

build_celily() {
    printf "Building celily...\n" >&2
    cargo build --release
}

install_celily() {
    printf "Installing celily...\n" >&2
    install -Dm755 "target/release/celily" /usr/bin/celily
    install -Dm644 "share/celily-mitmproxy.py" /usr/share/celily/celily-mitmproxy.py
}

main() {
    install_build_deps
    build_celily
    install_celily_deps
    install_celily
}

main "$@"
