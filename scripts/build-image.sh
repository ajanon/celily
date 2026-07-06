#!/bin/bash -eu
#
# Build a celily image for the given distro and publish it locally.
# Optionally export it for spread.
#
# Usage: build-image.sh --distro=<distro> [--export=<dir>]
#
#   --distro  One of: arch, ubuntu-22.04, ubuntu-24.04, debian-12
#   --export  Export the image to <dir> (spread layout).
#             Without this flag the image is only published locally.
#
# Requirements: LXD installed and initialised on the host.

declare -r DEFAULT_SHELL=/bin/bash
declare -r DEFAULT_USERNAME=celily
declare -r DEFAULT_UID=1000

declare distro=""
declare export_dir=""

parse_args() {
    for arg in "$@"; do
        case "${arg}" in
            --distro=?*)
                distro="${arg#*=}"
                ;;
            --export=?*)
                export_dir="${arg#*=}"
                ;;
            --help | -h)
                printf 'Usage: %s --distro=<distro> [--export=<dir>]\n' "${0##*/}" >&2
                printf '\nSupported distros: arch, ubuntu-22.04, ubuntu-24.04, debian-12\n' >&2
                exit 0
                ;;
            *)
                printf 'Unrecognised argument: %s\n' "${arg}" >&2
                printf 'Usage: %s --distro=<distro> [--export=<dir>]\n' "${0##*/}" >&2
                exit 1
                ;;
        esac
    done

    if [[ -z "${distro}" ]]; then
        printf 'Missing required argument: --distro\n' >&2
        exit 1
    fi
}

declare image_src

resolve_image_src() {
    case "${distro}" in
        arch)
            image_src="images:archlinux"
            ;;
        ubuntu-*)
            image_src="ubuntu:${distro#ubuntu-}"
            ;;
        debian-*)
            image_src="images:${distro}"
            ;;
        *)
            printf 'Unsupported distro: %s\n' "${distro}" >&2
            printf 'Supported: arch, ubuntu-*, debian-*\n' >&2
            exit 1
            ;;
    esac
}

install_packages() {
    printf '=> Installing required celily packages...\n' >&2
    case "${distro}" in
        arch)
            lxc_exec pacman -Syu --noconfirm ca-certificates >/dev/null
            ;;
        ubuntu-* | debian-*)
            lxc_exec bash -c "apt-get update -qq && apt-get install -y -qq ca-certificates"
            ;;
        *)
            printf 'Unsupported distro: %s\n.' "${DISTRO_ID}" >&2
            return 1
            ;;
    esac
}

clean_package_cache() {
    case "${distro}" in
        arch)
            lxc_exec bash -c "pacman -Scc --noconfirm >/dev/null && rm -rf /var/lib/pacman/sync"
            ;;
        ubuntu-* | debian-*)
            lxc_exec bash -c "apt-get clean && rm -rf /var/lib/apt/lists/*"
            ;;
        *)
            printf 'Unsupported distro: %s\n.' "${DISTRO_ID}" >&2
            return 1
            ;;
    esac
}

declare instance

lxc_exec() {
    lxc exec "${instance}" -- "$@"
}

wait_for_boot() {
    printf '=> Waiting for boot...\n'
    declare -i attempt=0
    declare -ri max_attempts=30
    declare -ri sleep_secs=2

    while ((attempt < max_attempts)); do
        declare status
        status="$(lxc exec "${instance}" -- systemctl is-system-running 2>/dev/null || true)"
        if [[ "${status}" =~ ^(running|degraded)$ ]]; then
            return 0
        fi
        ((attempt++)) || true
        sleep "${sleep_secs}"
    done

    printf 'Instance did not reach running state after %d attempts.\n' "${max_attempts}" >&2
    return 1
}

launch_instance() {
    printf '=> Launching %s as %s...\n' "${image_src}" "${instance}"
    lxc launch "${image_src}" "${instance}"
}

create_user() {
    printf '=> Creating user %s (uid %d)...\n' "${DEFAULT_USERNAME}" "${DEFAULT_UID}"
    lxc_exec useradd -m -u "${DEFAULT_UID}" -s "${DEFAULT_SHELL}" "${DEFAULT_USERNAME}"
}

instance_cleanup() {
    printf '=> Cleaning up...\n'

    clean_package_cache

    lxc_exec bash -eu <<EOF
# SSH host keys -- regenerated on first boot
rm -f /etc/ssh/ssh_host_*

# Machine ID -- systemd will regenerate
: > /etc/machine-id
rm -f /var/lib/dbus/machine-id

# Journal
journalctl --rotate 2>/dev/null || true
journalctl --vacuum-time=1s 2>/dev/null || true
rm -rf /var/log/journal/* 2>/dev/null || true

# Logs
find /var/log -type f -delete 2>/dev/null || true

# Temp files
rm -rf /tmp/* /var/tmp/* 2>/dev/null || true

# Shell history
rm -f /root/.bash_history /root/.zsh_history
rm -f "/home/${DEFAULT_USERNAME}/.bash_history" "/home/${DEFAULT_USERNAME}/.zsh_history" 2>/dev/null || true
EOF
}

stop_instance() {
    printf '=> Stopping %s...\n' "${instance}"
    lxc stop "${instance}"
}

publish_image() {
    printf '=> Publishing as %s...\n' "${alias}"
    lxc publish "${instance}" --alias "${alias}"
}

export_image() {
    printf '=> Exporting to %s/%s/...\n' "${export_dir}" "${alias}"
    lxc image export "${alias}" "${export_dir}/${alias}/"
}

main() {
    parse_args "$@"
    resolve_image_src

    declare -rg alias="celily-${distro}"
    instance="celily-build-${distro}"

    cleanup() {
        lxc delete --force "${instance}" 2>/dev/null || true
    }
    trap cleanup EXIT INT TERM

    # Remove any leftover from a previous failed run.
    lxc delete --force "${instance}" 2>/dev/null || true
    lxc image delete "${alias}" 2>/dev/null || true

    launch_instance
    wait_for_boot
    install_packages
    create_user
    instance_cleanup
    stop_instance
    publish_image

    if [[ -n "${export_dir}" ]]; then
        export_image
        printf '=> Done. Image "%s" published locally and exported.\n' "${alias}"
    else
        printf '=> Done. Image "%s" published locally.\n' "${alias}"
    fi
}

main "$@"
