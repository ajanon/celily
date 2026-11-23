"""The hosts the tests run on: Lima templates, and how to install what
the tests need in them. One module per distribution, implementing Host.

`vm` arguments are driver.Vm: run(argv, **kwargs) runs a command in it.
"""

import enum
from abc import ABC, abstractmethod

from backend.lxc import INCUS, LXD, Daemon

LXD_CHANNEL = "6/stable"


class HostBackend(enum.StrEnum):
    """The backend a host runs, and where it comes from: "distro" is the
    distribution's own package."""

    LXD_SNAP = "lxd-snap"
    LXD_DISTRO = "lxd-distro"
    INCUS_DISTRO = "incus-distro"

    @property
    def daemon(self) -> Daemon:
        match self:
            case HostBackend.LXD_SNAP | HostBackend.LXD_DISTRO:
                return LXD
            case HostBackend.INCUS_DISTRO:
                return INCUS


class Host(ABC):
    @property
    @abstractmethod
    def template(self) -> str:
        """The Lima template, e.g. "archlinux"."""

    @property
    @abstractmethod
    def backends(self) -> frozenset[HostBackend]:
        """The backends this host can run."""

    def upgrade(self, vm) -> None:
        """Bring a fresh VM up to date. A reboot follows."""

    @abstractmethod
    def install_backend(self, vm, backend: HostBackend) -> None:
        """Install `backend`, started at boot: a reboot follows."""

    @abstractmethod
    def install_uv(self, vm) -> None:
        """Python and uv, uv in the PATH of login shells."""


def install_lxd_snap(vm):
    vm.run(["sudo", "snap", "wait", "system", "seed.loaded"])
    # Installed already, maybe from another channel.
    installed = vm.run(["snap", "list", "lxd"], check=False, quiet=True).returncode == 0
    action = "refresh" if installed else "install"
    vm.run(["sudo", "snap", action, "lxd", f"--channel={LXD_CHANNEL}"])
