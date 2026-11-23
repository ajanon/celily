"""celily, run the way a user runs it, from a home directory of its own.

A whole HOME, not only XDG_CONFIG_HOME: celily reads HOME for mounts,
allowlists and the forbidden paths, and config outside HOME would be a
layout no user has (security tests would pass for the wrong reason).
"""

import contextlib
import dataclasses
import os
import re
import signal
import subprocess
import sys

import pytest
import tomli_w

from backend import Created, Kind

# Seconds. Below the pytest-timeout backstop in pyproject.toml.
RUN_TIMEOUT = 300

# celily's info log lines naming what it creates, in deletion order: the
# instance uses the bridge, the bridge uses the ACL. Names, not a diff of
# the backend before and after: a diff would also see the objects of other
# xdist workers, and of other users of a local LXD.
CREATED_LOG = {
    Kind.INSTANCE: re.compile(r"building instance name=(\S+)"),
    Kind.BRIDGE: re.compile(r"created isolation bridge bridge=(\S+)"),
    Kind.ACL: re.compile(r"created network ACL acl=(\S+)"),
}


def created_objects(log):
    """The backend objects a celily run created, from its info log."""
    return [
        Created(kind, match.group(1))
        for kind, pattern in CREATED_LOG.items()
        if (match := pattern.search(log))
    ]


@dataclasses.dataclass(frozen=True)
class Result:
    returncode: int
    stdout: str
    stderr: str
    created: list[Created]


class Celily:
    def __init__(self, *, binary, backend, cell, root, runtime_dir):
        self.binary = binary
        self.backend = backend
        self.home = root / "home"
        self.project = self.home / "project"
        self.config_dir = self.home / ".config" / "celily"
        # First in celily's PATH: fakes of the host tools it runs (rbw).
        self.bin = root / "bin"
        self.runtime_dir = runtime_dir
        # What the host and the cell provide.
        self.cell_fragment = {
            "distro": cell.distro,
            "image": cell.image,
            "vm": cell.kind == "vm",
            "backend": backend.config(),
        }
        # What the test wants: set it before run(). After the cell
        # fragment, so every run goes through merging.
        self.fragment = {}
        self.project.mkdir(parents=True)
        self.bin.mkdir()

    def _write_config(self):
        fragments = self.config_dir / "fragments"
        fragments.mkdir(mode=0o700, parents=True, exist_ok=True)
        # celily refuses any group/other bit, whatever the umask did.
        self.config_dir.chmod(0o700)
        fragments.chmod(0o700)
        for path, content in [
            (
                self.config_dir / "profiles.toml",
                {"directories": {"~": "it"}, "profiles": {"it": ["cell", "test"]}},
            ),
            (fragments / "cell.toml", self.cell_fragment),
            (fragments / "test.toml", self.fragment),
        ]:
            path.write_text(tomli_w.dumps(content))
            path.chmod(0o600)

    def run(self, *args, timeout=RUN_TIMEOUT):
        """Run celily with `args` in ~/project, then check it left nothing
        behind: what is left is deleted, and the test fails.

        For runs that launch an instance: one that exits 0 must log what it
        created (CREATED_LOG). A run that launches nothing (--help) needs
        another method."""
        self._write_config()
        # Nothing else from the caller leaks in (proxy variables, GIT_*).
        env = {
            "PATH": os.pathsep.join([str(self.bin), os.environ["PATH"]]),
            "HOME": str(self.home),
            "XDG_RUNTIME_DIR": str(self.runtime_dir),
            "RUST_LOG": "celily=info,celily_lib=info",
        }
        # Own process group: a timeout kills the lxc processes too.
        proc = subprocess.Popen(
            [self.binary, *args],
            cwd=self.project,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        timed_out = False
        try:
            stdout, stderr = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            _kill_group(proc)
            stdout, stderr = proc.communicate()
        except BaseException:
            # The pytest-timeout backstop, ^C: no log, so no sweep either.
            _kill_group(proc)
            raise
        # Captured: shown when the test fails, and kept in the junit report.
        sys.stderr.write(stderr)

        result = Result(proc.returncode, stdout, stderr, created_objects(stderr))
        # celily has no signal handling yet: after a timeout, this sweep is
        # what cleans up.
        left = [obj for obj in result.created if self.backend.exists(obj)]
        for obj in left:
            self.backend.delete(obj)
        # The runtime directory goes with the fixture.
        leaks = [str(obj) for obj in left]
        leaks += [str(path) for path in sorted(self.runtime_dir.iterdir())]
        if timed_out:
            pytest.fail(f"celily timed out after {timeout}s, leaked: {leaks}")
        if leaks:
            pytest.fail(f"celily left behind: {leaks}")
        # The leak check is only as good as the names celily logs. A run
        # that exited 0 created all of them: a missing one is a log line
        # that changed, which would turn the check into a silent no-op.
        logged = {obj.kind for obj in result.created}
        if proc.returncode == 0 and logged != set(Kind):
            missing = sorted(kind.value for kind in set(Kind) - logged)
            pytest.fail(f"celily exited 0 without logging a name for: {missing}")
        return result


def _kill_group(proc):
    with contextlib.suppress(ProcessLookupError):
        os.killpg(proc.pid, signal.SIGKILL)
