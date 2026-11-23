from hosts import Host, HostBackend, install_lxd_snap

APT = [
    "sudo",
    "DEBIAN_FRONTEND=noninteractive",
    "apt-get",
    "-o",
    "DPkg::Lock::Timeout=300",
]


class Ubuntu(Host):
    # The archive's lxd is a transitional package to the snap.
    backends = frozenset({HostBackend.LXD_SNAP})

    def __init__(self, version):
        self.version = version

    @property
    def template(self):
        return f"ubuntu-{self.version}"

    def install_backend(self, vm, backend):
        assert backend == HostBackend.LXD_SNAP, backend
        install_lxd_snap(vm)

    def install_uv(self, vm):
        vm.run([*APT, "update"])
        vm.run([*APT, "install", "--yes", "pipx"])
        # In ~/.local/bin, which ~/.profile adds to PATH.
        vm.run(["pipx", "install", "uv"])
