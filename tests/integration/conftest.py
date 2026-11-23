"""Options, cells and fixtures of the celily integration tests."""

import dataclasses
import os
import shutil
import tempfile
from pathlib import Path

import pytest

import image_build
from backend.lxd import Lxd
from runner import Celily


@dataclasses.dataclass(frozen=True)
class Cell:
    """A guest: what celily launches, as the `cell` fragment describes it."""

    distro: str
    kind: str  # "container" or "vm"

    def __str__(self):
        return f"{self.distro}-{self.kind}"

    @property
    def image(self):
        return image_build.alias(self.distro)


# Every test using the `celily` fixture runs once per cell: the cell is
# part of the test id (test_true[arch-container]), so -k selects cells.
# Containers only. VM cells need the host checked for KVM and LXD VM
# support (skip with the reason otherwise), and a VM image.
CELLS = [Cell("arch", "container")]


def pytest_addoption(parser):
    group = parser.getgroup("celily")
    group.addoption(
        "--celily",
        type=Path,
        # A separate PATH argument would be taken for a test path: pytest
        # picks its rootdir before it knows this option takes a value.
        help="the celily binary under test (required, as --celily=PATH)",
    )
    group.addoption(
        "--lxd-project",
        default="celily-it",
        help="the LXD project holding the guest images (default: %(default)s)",
    )
    group.addoption(
        "--lxd-pool",
        default="default",
        help="the storage pool of the instances' root disks (default: %(default)s)",
    )


@pytest.fixture(scope="session")
def celily_binary(pytestconfig):
    # No default: celily from PATH would test a stale binary silently.
    path = pytestconfig.getoption("--celily")
    if path is None:
        raise pytest.UsageError(
            "--celily=PATH is required: the celily binary under test"
        )
    if not os.access(path, os.X_OK):
        raise pytest.UsageError(f"--celily={path}: not an executable")
    # Absolute: every run has its own working directory.
    return path.resolve()


@pytest.fixture(scope="session")
def lxd(pytestconfig):
    return Lxd(
        project=pytestconfig.getoption("--lxd-project"),
        pool=pytestconfig.getoption("--lxd-pool"),
    )


@pytest.fixture(params=CELLS, ids=str)
def cell(request):
    return request.param


@pytest.fixture
def runtime_dir():
    """A fresh XDG_RUNTIME_DIR, under the real one: the proxy CA lands
    where it would for a user, the leak check sees only this test's."""
    real = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}")
    if not real.is_dir():
        raise pytest.UsageError(
            f"no runtime directory ({real}): run from a login session"
        )
    path = Path(tempfile.mkdtemp(prefix="celily-it-", dir=real))
    yield path
    # Not empty after a leak: the leak check failed the test already.
    shutil.rmtree(path)


@pytest.fixture
def celily(celily_binary, lxd, cell, tmp_path, runtime_dir):
    return Celily(
        binary=celily_binary,
        backend=lxd,
        cell=cell,
        root=tmp_path,
        runtime_dir=runtime_dir,
    )
