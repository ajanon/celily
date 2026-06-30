# celily

Launch ephemeral, network-isolated LXD/Incus containers (or VMs) for running
untrusted code safely. Every invocation gets a clean ephemeral environment.

celily assumes what is running inside the environment is untrusted. It has not
been hardened against an actively adversarial workload yet. The goal is to
defend against data exfiltration, communication with arbitrary hosts on the
internet, or accessing sensitive data (including API keys).

## Features

Most features are built with the following principles:

- deny-by-default approach: everything is forbidden, unless explicitly allowed.
- explicit is better than implicit

### Network isolation

Each environment gets its own virtual network bridge, where egress is denied by
default.

Only HTTP, DNS, and TCP can currently be in the network allowlist.

Secrets are injected at runtime, and never shared in the environment.

### Filesystem isolation

Each environment gets its own filesystem, isolated from the host by default.
Directories/files can be added to an allowlist and automatically bind-mount as
required (for configuration, additional projects, etc).

#### Worktree mode

In a git repository, `celily -w <name>` creates a git worktree inside the
container. `.git` is overlaid on the current directory read-write.

### Backend selection

celily supports both LXD and Incus via the `[backend]` config section:

```toml
[backend]
kind = "lxd"        # or "incus"
project = "celily"  # optional LXD/Incus project
pool = "zfs"        # storage pool (default: "default")
```

## Getting started

Prerequisites:

- [LXD](https://github.com/canonical/lxd) or
  [Incus](https://github.com/lxc/incus)
- [mitmproxy](https://github.com/mitmproxy/mitmproxy)
- Nightly Rust

Optional:

- [rbw](https://github.com/doy/rbw) for secrets

1. Build with nightly Rust

   ```bash
   cargo build --release
   ```

1. Build and publish a LXD/Incus image with the required packages (see
   `celily-config(5)` IMAGE REQUIREMENTS).

1. Create config at `~/.config/celily/config.toml`:

   ```toml
   distro = "arch"
   allowed_dirs = ["~/Projects"]
   allowed_files = []

   [network]

   # Allow GitHub Copilot API endpoints.
   [[network.allow]]
   type = "http"
   host = "api.business.githubcopilot.com"
   path_prefixes = [
       "/chat/completions",
       "/responses",
       "/v1/messages",
   ]
   auth = {
       secret = "github-copilot-token",
       header = "Authorization",
       prefix = "Bearer ",
   }
   headers = {
       "openai-intent" = "conversation-agent",
       "copilot-integration-id" = "copilot-developer-cli",
       "editor-version" = "copilot/1.0.56",
       "x-github-api-version" = "2026-06-01",
   }
   methods = ["POST"]
   ```

1. Run it:

   ```bash
   celily                                  # open a shell
   celily -- cargo build                   # run a command
   celily --profile rust -- cargo test     # use a profile
   celily -w feature-x                     # worktree mode
   celily --keep-instance                  # keep instance for debugging
   ```

## Security model

The host is assumed to be safe and not compromised. The only untrusted code and
input comes from software running inside the isolated environment.

### Config isolation

All configuration lives in `~/.config/celily/`. This directory is permanently
forbidden from bind-mounts. There is no per-project config file, and no
environment variable that can redirect the config path.

Note: this assumes XDG_CONFIG_DIR or HOME are not compromised.

### Mount security

Every mount source is validated against an allowlist before the container starts
and mounted read-only by default and:

- Must be under the current user's `$HOME`.
- Must be listed exactly in `allowed_dirs` (for directories) or `allowed_files`
  (for files). Subtrees are not implicitly allowed.
- Several paths are permanently forbidden, to avoid mistakes:
  - `$HOME`, `~/.config`, `~/.local`, `~/.local/share` (subdirs or individual
    files are allowed mounted)
  - `~/.config/celily`
  - `~/.ssh`, `~/.gnupg`, `~/.local/share/keyrings`
- Symlinks, sockets, and special files are rejected.

### Network isolation

Every invocation creates a dedicated network bridge with an egress ACL and a
per-instance MITM proxy.

- A dedicated bridge is created with an nftables ACL that blocks all TCP egress
  except connections to the proxy (proxy IP available inside the container via
  HTTP_PROXY and HTTPS_PROXY).
- A per-instance mitmdump process enforces an HTTP host/path allowlist and
  injects authentication headers where configured.
- TCP rules allow raw connections to specific IP/port
- DNS filtering is on by default: mitmproxy only answers for hosts on the same
  host allowlist as HTTP.

### Secrets

Authentication secrets (API keys, tokens) are resolved from Bitwarden via `rbw`
and injected as HTTP headers by the MITM proxy.

## Documentation

Full documentation lives in the man pages:

- `man/celily.1` -- CLI flags, mount security, environment handling, worktree
  mode
- `man/celily-config.5` -- config file schema, sections, merging rules, profile
  inheritance, image requirements
