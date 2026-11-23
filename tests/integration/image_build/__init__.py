"""The guest images: what celily launches in the tests.

Every image has the CA trust tools, the tools the tests run (bash and
curl), a user `dev` (1000:1000, celily's defaults), and boots to the
running state (celily-config(5), IMAGE REQUIREMENTS). One module per
distribution, implementing Guest; build() does the steps they share.
"""

import dataclasses
import time
from abc import ABC, abstractmethod

from command import Failed, run

USER = "dev"
UID = GID = 1000
# What `systemctl is-system-running --wait` prints once the boot is over.
BOOTED = {"running", "degraded", "maintenance"}


def alias(distro):
    """The image of a guest, by celily's `distro` value."""
    return f"celily-it-{distro}"


@dataclasses.dataclass(frozen=True)
class Instance:
    """The instance an image is built from."""

    cli: list[str]  # lxc or incus, maybe behind a prefix
    name: str
    project: str

    def run(self, argv, **kwargs):
        return run(
            [*self.cli, "exec", self.name, "--project", self.project, "--", *argv],
            **kwargs,
        )


class Guest(ABC):
    @property
    @abstractmethod
    def distro(self) -> str:
        """celily's `distro` value."""

    @property
    @abstractmethod
    def source(self) -> str:
        """The image it is built from, e.g. "images:archlinux"."""

    def fix_boot(self, instance: Instance) -> None:
        """Make it boot to the running state: mask what fails in a
        container."""

    @abstractmethod
    def wait_for_network(self, instance: Instance) -> None:
        """Booted is not online: wait until packages can be installed."""

    @abstractmethod
    def install_ca_tools(self, instance: Instance) -> None:
        """What celily needs to add its CA to the trust store."""

    @abstractmethod
    def install_test_tools(self, instance: Instance) -> None:
        """What the tests run in the instance: bash and curl."""

    def add_user(self, instance: Instance) -> None:
        instance.run(["groupadd", f"--gid={GID}", USER])
        instance.run(["useradd", "--create-home", f"--uid={UID}", f"--gid={GID}", USER])


def build(guest, *, cli, project, pool, network):
    """Build the image of `guest` in `project`, replacing any previous one.
    `cli` is LXD's or Incus' (same commands): ["lxc"], ["incus"], or
    behind a prefix running it elsewhere."""
    if run(
        [*cli, "query", f"/1.0/projects/{project}"], check=False, quiet=True
    ).returncode:
        run([*cli, "project", "create", project, "--config=features.images=true"])
    instance = Instance(cli, f"celily-it-build-{guest.distro}", project)
    in_project = ["--project", project]
    # The project's default profile is empty: no root disk, no NIC.
    run(
        [
            *cli,
            "launch",
            guest.source,
            instance.name,
            *in_project,
            f"--storage={pool}",
            f"--network={network}",
        ]
    )
    try:
        # Booted, maybe degraded: fix_boot comes next.
        _wait_for_boot(instance)
        guest.fix_boot(instance)
        instance.run(["systemctl", "reset-failed"])
        state = instance.run(
            ["systemctl", "is-system-running"], check=False, capture=True
        ).stdout.strip()
        if state != "running":
            instance.run(["systemctl", "--failed"], check=False)
            raise Failed(f"{guest.source} boots {state}, celily needs running")
        guest.wait_for_network(instance)
        guest.install_ca_tools(instance)
        guest.install_test_tools(instance)
        guest.add_user(instance)
        # Generated again on first boot, one per instance.
        instance.run(["truncate", "--size=0", "/etc/machine-id"])
        run([*cli, "stop", instance.name, *in_project])
        name = alias(guest.distro)
        query = f"/1.0/images/aliases/{name}?project={project}"
        if run([*cli, "query", query], check=False, quiet=True).returncode == 0:
            run([*cli, "image", "delete", name, *in_project])
        run([*cli, "publish", instance.name, *in_project, f"--alias={name}"])
    finally:
        run([*cli, "delete", "--force", instance.name, *in_project], check=False)


def _wait_for_boot(instance, timeout=120):
    """Until the boot is over, whatever its outcome. `launch` returns
    before systemd listens: --wait waits for the boot, not for the bus."""
    deadline = time.monotonic() + timeout
    while True:
        proc = instance.run(
            ["systemctl", "is-system-running", "--wait"],
            check=False,
            capture=True,
            quiet=True,
        )
        if proc.stdout.strip() in BOOTED:
            return
        if time.monotonic() > deadline:
            why = proc.stdout.strip() or proc.stderr.strip()
            raise Failed(f"{instance.name} not booted after {timeout}s: {why}")
        time.sleep(1)
