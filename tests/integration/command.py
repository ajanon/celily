"""Commands the driver and the image build run, shown as they run."""

import shlex
import subprocess
import sys


class Failed(Exception):
    pass


def run(argv, *, check=True, capture=False, quiet=False):
    """Run `argv`, stdin from /dev/null. With `capture`, stdout is
    returned (as text) instead of shown. `quiet` hides the command line:
    for probes run in a loop, or whose failure is expected."""
    if not quiet:
        print("+", shlex.join(argv), file=sys.stderr, flush=True)
    proc = subprocess.run(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE if capture else None,
        text=True,
        check=False,
    )
    if check and proc.returncode != 0:
        raise Failed(f"exit {proc.returncode}: {shlex.join(argv)}")
    return proc
