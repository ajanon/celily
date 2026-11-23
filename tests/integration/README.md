# Integration tests

celily tested as a black box: the binary, run the way a user runs it, against a
real LXD or Incus. `cargo test` covers the logic; these tests cover what only
shows up with a real backend: instances, bridges, ACLs, the proxy, and whatever
is left behind.

## Requirements

pytest runs where the backend runs: it checks what celily leaves behind there.

- LXD or Incus, usable without sudo.
- A login session, with an `XDG_RUNTIME_DIR`.
- Python and uv.
- A project (default `celily-it`) with `features.images=true`, holding the guest
  image under an alias (default `celily-it-arch`).

Guest images (`celily-it-<distro>`, e.g. `celily-it-arch`) have the CA trust
tools, bash and curl (the tests run them), and a user `dev` (uid and gid 1000)
with a home directory, and must boot to the `running` state (see
`celily-config(5)`, IMAGE REQUIREMENTS). `driver.py image` builds them with your
`lxc` (or `incus`, with `--backend=incus`), replacing previous ones (`--help`
for the guests, project, pool and network). How is in `image_build/`, one module
per guest distribution.

## Running

```
cargo build -p celily
cd tests/integration
uv run pytest --celily=../../target/debug/celily
```

- `--celily=PATH`: the celily binary under test. Required, with no default
  (whatever `celily` is on `PATH` may be stale), and written with the `=`:
  pytest picks its rootdir before it knows the option takes a value, so a
  separate `PATH` is taken for a test path.
- `--backend lxd|incus`: the daemon celily runs instances on, as in the `kind`
  of its `[backend]`. Default: `lxd`.
- `--backend-project NAME`: the project holding the images. Default:
  `celily-it`.
- `--backend-pool NAME`: the storage pool of the instances' root disks. Default:
  `default`.
- `-n auto`: run tests in parallel (pytest-xdist).
- `-k arch-container`: only the tests of a cell (see below).

With snap LXD, also pass a `--basetemp` under your home directory, in a path
with no hidden component: snap's `lxc` has a private `/tmp` and no access to
hidden directories in `HOME`.

### In a VM

What CI does, per host (a Lima template) and backend. Needs Docker and Lima, no
backend of your own:

```
python3 -m builder archlinux
tests/integration/driver.py up archlinux --backend=lxd-distro
tests/integration/driver.py test archlinux --backend=lxd-distro \
    --celily=target/archlinux/debug/celily -- -n auto
tests/integration/driver.py down archlinux --backend=lxd-distro
```

Hosts: `archlinux` (`--backend=lxd-distro` or `incus-distro`) and `ubuntu-24.04`
(`--backend=lxd-snap`), one module each in `hosts/`. The backend says where it
comes from: `distro` is the distribution's package.

`python3 -m builder` (from the repository root) builds celily in a container of
the host's distribution, into `target/<host>`. `up` starts the VM
`celily-it-<host>-<backend>`, installs the backend and builds the guest images
in it. `test` copies the tests and the binary in, and runs pytest there with the
arguments after `--`; `--junitxml=PATH` copies its junit report back, failed
tests or not. The VM stays until `down`: `test` can run again, and
`limactl shell celily-it-<host>-<backend>` gets you in.

`summary.py JUNIT` sums a report up in Markdown: the counts, and every test that
did not pass with its message. CI makes it the job summary, and keeps the report
itself as an artifact for 7 days: every test's output is in it, celily's log
included.

## Writing tests

The `celily` fixture gives each test its own home directory, with its own celily
config, and runs celily from there the way a user does:

- `celily.fragment`: the test's config fragment, a dict written as TOML before
  each run. It comes after the `cell` fragment (distro, image, backend), so it
  can override it.
- `celily.run(*args)`: runs celily in `~/project` (`celily.project`) and returns
  `returncode`, `stdout`, `stderr`, and `created` (the instance, bridge and ACL
  celily logged creating).
- After every run, `run()` checks that nothing is left behind (instance, bridge,
  ACL, files in the runtime directory). Leftovers are deleted and the test
  fails. The check goes by the names celily logs: a run that exits 0 without
  logging all three fails too.
- celily's log goes to the captured output: it shows when a test fails, and in
  the junit report.
- `celily.bin`: first in celily's `PATH`, for fakes of the host tools it runs
  (`test_network.py` puts an `rbw` there).

The `http_server` fixture is an HTTP server on the host's loopback
(`servers.py`): the instance reaches it only through celily's proxy. It records
every connection and every request, so a test sees both what the proxy let
through and what it did not. Rules name it by `host` and `port`; `url(path)`
gives the URL to fetch.

Tests run once per cell (guest distribution x instance kind): `arch-container`
for now. `skip` always carries a reason; known bugs are `xfail(strict=True)`,
which fails once the bug is fixed.
