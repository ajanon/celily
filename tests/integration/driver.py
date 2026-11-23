#!/usr/bin/env python3
"""The VMs the integration tests run in, one per host (a Lima template)
and backend; and the tests, run in them.

    driver.py up HOST --backend=B         start it, backend and images in
    driver.py test HOST --backend=B --celily=PATH [--junitxml=PATH]
                                          [-- PYTEST_ARGS...]
    driver.py down HOST --backend=B
    driver.py image [OPTIONS]             guest images, with your lxc or incus

The binary under test comes from elsewhere: `python3 -m builder HOST`
builds one for the host. CI runs the same commands
(.github/workflows/integration.yml). Standard library only, so far: CI
runs it with the runner's python3, nothing installed.
"""

import argparse
import shlex
import subprocess
import sys
import tarfile
from pathlib import Path

import hosts.arch
import hosts.ubuntu
import image_build
import image_build.arch
from backend.lxc import DAEMONS
from command import Failed, run
from hosts import HostBackend

HERE = Path(__file__).resolve().parent

HOSTS = {
    host.template: host for host in [hosts.arch.Arch(), hosts.ubuntu.Ubuntu("24.04")]
}
GUESTS = {guest.distro: guest for guest in [image_build.arch.Arch()]}
PROJECT = "celily-it"


class Vm:
    def __init__(self, host, backend):
        self.host = host
        self.backend = backend
        self.name = f"celily-it-{host.template}-{backend}"

    def limactl(self, command, *args):
        run(["limactl", "--tty=false", command, *args])

    def shell(self, workdir="."):
        """A prefix running a command in the VM, as its user, in a login
        shell. A relative `workdir` is from the user's home."""
        return ["limactl", "--tty=false", "shell", f"--workdir={workdir}", self.name]

    def run(self, argv, *, workdir=".", **kwargs):
        return run([*self.shell(workdir), *argv], **kwargs)

    def output(self, argv):
        return self.run(argv, capture=True).stdout

    def extract(self, files, dest):
        """Copy `files` ({name in dest: local path}) to `dest`, keeping
        their modes. Streamed: the debug binary weighs hundreds of MB."""
        argv = [*self.shell(), "tar", "--extract", f"--directory={dest}"]
        print("+", shlex.join(argv), "< tar", file=sys.stderr, flush=True)
        # Unbuffered: closing stdin flushes nothing into a broken pipe.
        with subprocess.Popen(argv, stdin=subprocess.PIPE, bufsize=0) as proc:
            try:
                with tarfile.open(fileobj=proc.stdin, mode="w|") as tar:
                    for name, path in files.items():
                        tar.add(path, arcname=name)
            except BrokenPipeError:
                pass  # tar exited early: its status says why
        if proc.returncode != 0:
            raise Failed(f"exit {proc.returncode}: extracting into {self.name}:{dest}")


def up(args):
    vm = _vm(args)
    daemon = vm.backend.daemon
    vm.limactl("start", "--plain", f"--name={vm.name}", f"template:{vm.host.template}")
    vm.host.upgrade(vm)
    vm.host.install_backend(vm, vm.backend)
    vm.host.install_uv(vm)
    user = vm.output(["id", "-un"]).strip()
    # Missing on some hosts, maybe: harmless where it exists. The daemon
    # gives its socket to the group when it starts, after the reboot.
    vm.run(["sudo", "groupadd", "--force", "--system", daemon.group])
    vm.run(["sudo", "usermod", "--append", f"--groups={daemon.group}", user])
    # The kernel an upgrade installed, and sessions in the group (the ssh
    # control master would keep the old groups).
    vm.limactl("restart", vm.name)
    groups = vm.output(["id", "-nG"]).split()
    if daemon.group not in groups:
        raise Failed(f"{user} not in {daemon.group} after a reboot: {groups}")
    vm.run(["sudo", *daemon.admin, "waitready", "--timeout=120"])
    vm.run(["sudo", *daemon.admin, "init", "--auto"])
    for guest in GUESTS.values():
        image_build.build(
            guest,
            cli=[*vm.shell(), daemon.cli],
            project=PROJECT,
            pool="default",
            network=daemon.network,
        )


def test(args):
    vm = _vm(args)
    vm.run(["rm", "-rf", "it", "it-tmp", "it-junit.xml"])
    vm.run(["mkdir", "it"])
    vm.extract({**_tests(), "celily": args.celily}, "it")
    # --basetemp: under HOME, no hidden component, for snap's lxc.
    argv = [
        "uv",
        "run",
        "pytest",
        "--celily=celily",
        f"--backend={vm.backend.daemon.kind}",
        "--basetemp=../it-tmp",
        "--junitxml=../it-junit.xml",
    ]
    code = vm.run([*argv, *args.pytest_args], workdir="it", check=False).returncode
    if args.junitxml:
        # Never a stale one, from an earlier run.
        args.junitxml.unlink(missing_ok=True)
        args.junitxml.parent.mkdir(parents=True, exist_ok=True)
        try:
            vm.limactl("copy", f"{vm.name}:it-junit.xml", str(args.junitxml))
        except Failed as err:
            # pytest stopped before writing it: its output says why.
            print(f"driver: no junit report: {err}", file=sys.stderr)
            return code or 1
    return code


def down(args):
    vm = _vm(args)
    vm.limactl("delete", "--force", vm.name)


def image(args):
    daemon = DAEMONS[args.backend]
    for distro in args.guest or GUESTS:
        image_build.build(
            GUESTS[distro],
            cli=[daemon.cli],
            project=args.backend_project,
            pool=args.backend_pool,
            network=args.network or daemon.network,
        )


def _vm(args):
    host = HOSTS[args.host]
    backend = HostBackend(args.backend)
    if backend not in host.backends:
        supported = ", ".join(sorted(host.backends))
        raise Failed(f"no {backend} on {host.template} (supported: {supported})")
    return Vm(host, backend)


def _tests():
    """This directory, without caches and virtual environments."""
    files = {}
    for root, dirs, names in HERE.walk():
        dirs[:] = [d for d in dirs if not d.startswith(".") and d != "__pycache__"]
        for name in names:
            path = root / name
            files[str(path.relative_to(HERE))] = path
    return files


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    commands = parser.add_subparsers(required=True)

    def vm_command(name, func, **kwargs):
        sub = commands.add_parser(name, **kwargs)
        sub.add_argument("host", choices=HOSTS)
        sub.add_argument("--backend", choices=list(HostBackend), required=True)
        sub.set_defaults(func=func)
        return sub

    vm_command("up", up)
    vm_command("down", down)
    sub = vm_command("test", test)
    sub.add_argument("--celily", type=Path, required=True, help="the binary to test")
    sub.add_argument(
        "--junitxml",
        type=Path,
        help="copy pytest's junit report there, passed or failed",
    )
    sub.add_argument("pytest_args", nargs="*", help="after --, passed to pytest")

    sub = commands.add_parser("image", help="build guest images with your own CLI")
    sub.add_argument(
        "--guest", action="append", choices=GUESTS, help="repeatable (default: all)"
    )
    sub.add_argument("--backend", choices=sorted(DAEMONS), default="lxd")
    sub.add_argument("--backend-project", default=PROJECT)
    sub.add_argument("--backend-pool", default="default")
    sub.add_argument(
        "--network",
        help="the build instance's (default: the bridge `init --auto` creates)",
    )
    sub.set_defaults(func=image)

    args = parser.parse_args()
    try:
        return args.func(args) or 0
    except Failed as err:
        print(f"driver: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
