from hosts import Host, HostBackend

# Below the default ranges of regular users (100000 + 65536 each).
ROOT_SUBIDS = "1000000-1000999999"


class Arch(Host):
    template = "archlinux"
    # No LXD_SNAP yet: snapd is in the AUR only (pin it when it comes).
    backends = frozenset({HostBackend.LXD_DISTRO})

    def upgrade(self, vm):
        # The cloud image is older than the keyring, and than the glibc
        # celily is built against.
        vm.run(
            ["sudo", "pacman", "-Sy", "--noconfirm", "--needed", "archlinux-keyring"]
        )
        vm.run(["sudo", "pacman", "-Su", "--noconfirm"])

    def install_backend(self, vm, backend):
        assert backend == HostBackend.LXD_DISTRO, backend
        vm.run(["sudo", "pacman", "-S", "--noconfirm", "--needed", "lxd"])
        # Read by LXD when it starts.
        vm.run(
            [
                "sudo",
                "usermod",
                f"--add-subuids={ROOT_SUBIDS}",
                f"--add-subgids={ROOT_SUBIDS}",
                "root",
            ]
        )
        # Not --now: the upgrade removed the running kernel's modules.
        vm.run(["sudo", "systemctl", "enable", "lxd.service"])

    def install_uv(self, vm):
        vm.run(["sudo", "pacman", "-S", "--noconfirm", "--needed", "python", "uv"])
