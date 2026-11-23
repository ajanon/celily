import subprocess
import sys

from backend import Backend, Kind

LXC = "lxc"


class Lxd(Backend):
    def __init__(self, *, project, pool):
        self.project = project
        self.pool = pool

    def config(self):
        return {"kind": "lxd", "project": self.project, "pool": self.pool}

    def _lxc(self, *args):
        return subprocess.run(
            [LXC, *args],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            check=False,
        )

    def exists(self, obj):
        # Bridges and ACLs live in the default project: celily creates
        # them there (non-default projects have no bridge networks).
        match obj.kind:
            case Kind.INSTANCE:
                path = f"/1.0/instances/{obj.name}?project={self.project}"
            case Kind.BRIDGE:
                path = f"/1.0/networks/{obj.name}"
            case Kind.ACL:
                path = f"/1.0/network-acls/{obj.name}"
        proc = self._lxc("query", path)
        if proc.returncode == 0:
            return True
        if "not found" in proc.stderr.lower():
            return False
        raise RuntimeError(f"{LXC} query {path}: {proc.stderr.strip()}")

    def delete(self, obj):
        match obj.kind:
            case Kind.INSTANCE:
                args = ["delete", "--force", "--project", self.project, obj.name]
            case Kind.BRIDGE:
                args = ["network", "delete", obj.name]
            case Kind.ACL:
                args = ["network", "acl", "delete", obj.name]
        proc = self._lxc(*args)
        if proc.returncode != 0:
            print(
                f"sweep: {LXC} {' '.join(args)}: {proc.stderr.strip()}", file=sys.stderr
            )
