"""celily, built for a distribution in a container of it: the binary links
against that distribution's libraries, as it would for its users.

    python3 -m builder DISTRO [--out DIR]

One module per distribution family, implementing Distro. Standard library
only, so far: CI runs it with the runner's python3, nothing installed.
"""

import os
import shlex
import subprocess
import sys
from abc import ABC, abstractmethod
from pathlib import Path


class Failed(Exception):
    pass


class Distro(ABC):
    """A distribution celily is built for."""

    @property
    @abstractmethod
    def name(self) -> str:
        """As given on the command line, e.g. "ubuntu-24.04"."""

    @property
    @abstractmethod
    def oci_image(self) -> str:
        """The image of the build container."""

    @abstractmethod
    def setup(self) -> list[list[str]]:
        """Commands run as root in the container, installing a C compiler
        and Rust."""

    def env(self) -> dict[str, str]:
        """Environment of every command in the container."""
        return {}

    def cargo(self) -> str:
        """Cargo, once set up."""
        return "cargo"


def build(distro: Distro, *, repo: Path, out: Path, engine: str = "docker") -> Path:
    """Build celily from `repo` (mounted read-only) into `out`, with the
    toolchain and dependencies downloaded there too: one directory to
    cache. Returns the binary."""
    out.mkdir(parents=True, exist_ok=True)
    env = {"CARGO_TARGET_DIR": "/out", "CARGO_HOME": "/out/cargo", **distro.env()}
    container = _run(
        [
            engine,
            "run",
            "--detach",
            "--rm",
            f"--volume={repo}:/src:ro",
            f"--volume={out}:/out",
            *(f"--env={key}={value}" for key, value in env.items()),
            distro.oci_image,
            "sleep",
            "infinity",
        ],
        capture=True,
    ).strip()
    try:
        for argv in distro.setup():
            _run([engine, "exec", container, *argv])
        # As the caller: `out` stays theirs.
        _run(
            [
                engine,
                "exec",
                f"--user={os.getuid()}:{os.getgid()}",
                "--workdir=/src",
                container,
                distro.cargo(),
                "build",
                "--locked",
                "--package=celily",
            ]
        )
    finally:
        _run([engine, "rm", "--force", container], capture=True)
    return out / "debug" / "celily"


def _run(argv, capture=False):
    print("+", shlex.join(argv), file=sys.stderr, flush=True)
    proc = subprocess.run(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE if capture else None,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise Failed(f"exit {proc.returncode}: {shlex.join(argv)}")
    return proc.stdout
