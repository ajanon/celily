#!/bin/bash -eu
#
# Install and initialize LXD on a test or CI host.
#
# Usage: setup-lxd.sh [--channel=<snap-channel>]
#
#   --channel  Snap channel for LXD (Ubuntu only).
#              Defaults to ${LXD_SNAP_CHANNEL:-latest/stable}.
#
# When SPREAD_PATH is set (spread context), also imports a pre-built
# celily image if one exists. In CI, images are built separately.

declare -r SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "${SCRIPT_DIR}/utils.sh"

declare channel="latest/stable"

parse_args() {
    for arg in "$@"; do
        case "${arg}" in
            --channel=?*)
                channel="${arg#*=}"
                ;;
            --help | -h)
                printf 'Usage: %s [--channel=<snap-channel>]\n' "${0##*/}" >&2
                exit 0
                ;;
            *)
                printf 'Unrecognised argument: %s\n' "${arg}" >&2
                printf 'Usage: %s [--channel=<snap-channel>]\n' "${0##*/}" >&2
                exit 1
                ;;
        esac
    done
}

main() {
    parse_args "$@"

    case "${DISTRO_ID}" in
        ubuntu)
            snap_install lxd --channel="${channel}"
            ;;
        *)
            package_install lxd
            ;;
    esac

    if [[ "${DISTRO_ID}" = arch ]]; then
        systemctl start lxd 2>/dev/null || true
    fi

    lxd init --auto
    lxd waitready --timeout=30

    # Configure firewall
    if sudo iptables -nL DOCKER-USER; then
        sudo iptables  -I DOCKER-USER -i lxdbr0 -j ACCEPT
        sudo iptables  -I DOCKER-USER -o lxdbr0 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
    fi
    if sudo ip6tables -nL DOCKER-USER; then
        sudo ip6tables -I DOCKER-USER -i lxdbr0 -j ACCEPT
        sudo ip6tables -I DOCKER-USER -o lxdbr0 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
    fi

    if [[ "${SUDO_USER}" != "root" ]] && getent group lxd &>/dev/null; then
        usermod -aG lxd "${SUDO_USER}"
    fi
}

main "$@"
