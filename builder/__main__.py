import argparse
import sys
from pathlib import Path

from builder import Failed, build
from builder.arch import Arch
from builder.ubuntu import Ubuntu

DISTROS = {distro.name: distro for distro in [Arch(), Ubuntu("24.04", rust="1.97")]}
REPO = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(
        prog="python3 -m builder", description="Build celily for a distribution."
    )
    parser.add_argument("distro", choices=DISTROS)
    parser.add_argument(
        "--out", type=Path, help="the output directory (default: target/DISTRO)"
    )
    args = parser.parse_args()
    out = args.out or REPO / "target" / args.distro
    try:
        binary = build(DISTROS[args.distro], repo=REPO, out=out.resolve())
    except Failed as err:
        print(f"builder: {err}", file=sys.stderr)
        return 1
    print(binary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
