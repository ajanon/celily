"""LXD and Incus: one CLI with two names, as celily's LxcBackend has it."""

import dataclasses
import subprocess
import sys

from backend import Backend, Kind


@dataclasses.dataclass(frozen=True)
class Daemon:
    """What tells LXD and Incus apart, to celily and to their admin."""

    kind: str  # celily's [backend] kind
    cli: str
    admin: tuple[str, ...]  # the daemon's own commands: waitready, init
    group: str  # gets the socket: its members need no sudo
    network: str  # the bridge `init --auto` creates


LXD = Daemon(kind="lxd", cli="lxc", admin=("lxd",), group="lxd", network="lxdbr0")
INCUS = Daemon(
    kind="incus",
    cli="incus",
    admin=("incus", "admin"),
    group="incus-admin",
    network="incusbr0",
)
DAEMONS = {daemon.kind: daemon for daemon in [LXD, INCUS]}


class Lxc(Backend):
    def __init__(self, daemon, *, project, pool):
        self.daemon = daemon
        self.project = project
        self.pool = pool

    def config(self):
        return {"kind": self.daemon.kind, "project": self.project, "pool": self.pool}

    def _cli(self, *args):
        return subprocess.run(
            [self.daemon.cli, *args],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            check=False,
        )

    def exists(self, obj):
        # Bridges and ACLs live in the default project: celily creates
        # them there (non-default LXD projects have no bridge networks).
        match obj.kind:
            case Kind.INSTANCE:
                path = f"/1.0/instances/{obj.name}?project={self.project}"
            case Kind.BRIDGE:
                path = f"/1.0/networks/{obj.name}"
            case Kind.ACL:
                path = f"/1.0/network-acls/{obj.name}"
        proc = self._cli("query", path)
        if proc.returncode == 0:
            return True
        if "not found" in proc.stderr.lower():
            return False
        raise RuntimeError(f"{self.daemon.cli} query {path}: {proc.stderr.strip()}")

    def delete(self, obj):
        match obj.kind:
            case Kind.INSTANCE:
                args = ["delete", "--force", "--project", self.project, obj.name]
            case Kind.BRIDGE:
                args = ["network", "delete", obj.name]
            case Kind.ACL:
                args = ["network", "acl", "delete", obj.name]
        proc = self._cli(*args)
        if proc.returncode != 0:
            print(
                f"sweep: {self.daemon.cli} {' '.join(args)}: {proc.stderr.strip()}",
                file=sys.stderr,
            )
