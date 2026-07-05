#!/bin/bash -eu

. /etc/os-release 2>/dev/null || true
declare -r DISTRO_ID="${ID:-unknown}"

package_install() {
    case "${DISTRO_ID}" in
        alpine)
            apk add "$@"
            ;;
        arch)
            pacman -Syuq --noconfirm "$@"
            ;;
        debian | devuan | kali | ubuntu)
            apt-get update -qq
            apt-get install -yqq "$@"
            ;;
        fedora)
            dnf install -yq "$@"
            ;;
        opensuse* | sles)
            zypper -qn install "$@"
            ;;
        void)
            xbps-install -Syu "$@"
            ;;
        *)
            printf "Unsupported distro for package install: %s\n" "${DISTRO_ID}" >&2
            return 1
            ;;
    esac
}

snap_install() {
    declare -r name="$1"
    if snap list "${name}" &>/dev/null; then
        snap refresh "${name}" "${@:2}"
    else
        snap install "${name}" "${@:2}"
    fi
}
