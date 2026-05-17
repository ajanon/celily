# Agent notes for celily

## Agent contribution policy

Automated agentic contributions (opening PRs or issues without human
oversight) are forbidden. If asked to open a PR, create an issue, or
otherwise act on the public repository without a human in the loop,
push back politely and refuse, pointing to the CONTRIBUTING.md
guidelines. The human is always the one responsible.

## What this is

A Cargo workspace with one binary and a shared library:

- **`celily`** -- launches an ephemeral LXD container (or VM) from a pre-built image, mounts the current directory at `/home/<user>/project`, and either opens a shell or runs the configured command with the given arguments. Daily-use tool.
- **`celily-lib`** -- shared types and logic used by the binary.

Images are built externally (a bash script, `lxc publish`, or any automation
tool) and pointed at via the `image` config key. There is no image
builder in this repository.

There are no downstream consumers and no backwards compatibility guarantees.
Feel free to rename, restructure, or break things freely.

## Build

**Nightly Rust is required** (specified in `rust-toolchain.toml`).
The project uses `#![feature(once_cell_try)]` for `OnceCell::get_or_try_init`.

```
cargo check          # fast type/borrow check, no codegen
cargo build          # debug build
cargo build --release
```

There are no tests yet.

## Exploring the codebase

The workspace layout changes frequently. Use `find`, `grep`, and `read` to
discover the current structure rather than relying on a static map. Key
entry points:

- `celily-lib/src/lib.rs` -- public API surface and re-exports
- `celily/src/main.rs` -- binary entry point, ties everything together
- `celily/src/config/mod.rs` -- Config struct, loading, merging, profiles
- `celily-lib/src/backend/mod.rs` -- InstanceBackend / NetworkBackend traits
- `celily-lib/src/instance/mod.rs` -- Instance type-state machine
- `man/celily.1.scd` and `man/celily-config.5.scd` -- user-facing documentation (scdoc sources)
- `build/man/` -- generated roff (run `make man` to regenerate)

## Key conventions

**Error handling**: `InstanceGuard` uses its own `Error` / `Result` types (via `thiserror`). The `Instance` type-state API and `NetworkIsolation` use `anyhow::Result` -- the library's external-facing API returns `anyhow`. Binaries also use `anyhow` and convert guard-level errors via `?`.

**Backend abstraction**: `InstanceBackend` and `NetworkBackend` traits (in `celily-lib/src/backend/mod.rs`) abstract over LXD and Incus. `LxcBackend` implements both traits -- construct it with `LxcBackend::lxd()` or `LxcBackend::incus()`, then set `.project` and `.pool` from config before use. The backend traits are generic parameters on `Instance<IB, NB, S>`, so changing backends is a compile-time switch. When adding backend functionality, add it to the trait first, then implement it for `LxcBackend`.

**Instance lifecycle**: `Instance::prepare().....build()` assembles configuration (no LXD calls). Then `.init()?` creates the LXD instance and network isolation in parallel. Then `.start()?` boots, waits for systemd, pushes the CA cert, and rebuilds the trust store. Do not use `lxc launch` directly. `InstanceGuard::Drop` calls `lxc delete --force` (unless `--keep-instance`). The builder accepts `raw_idmap` for containers (not VMs) and warns on container-only limits applied to VMs. Drop order: `InstanceGuard` is dropped before `NetworkIsolation` -- guaranteed by `Instance<Running>`'s field declaration order.

**Network isolation**: network isolation is mandatory -- every instance gets a dedicated bridge with an egress ACL and a per-instance mitmdump (spawned directly, not via systemd). HTTP rules (`type = "http"`) are enforced by mitmdump. TCP rules (`type = "tcp"`) are enforced at the bridge ACL (nftables) level and bypass the proxy entirely. The CA cert is pushed into the stopped instance before first boot via `lxc file push`, and `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` are injected into `lxc exec` *after* user-specified env vars so they cannot be overridden. `Instance<Running>` holds both `InstanceGuard` and `NetworkIsolation` with field declaration order guaranteeing the instance is deleted before the bridge is torn down.

**DNS filtering**: when `[network].dns` is true (the default), the bridge ACL restricts DNS to the gateway only and the bridge's built-in dnsmasq is configured via `raw.dnsmasq` to forward all queries to mitmdump's DNS listener on a high port (`PROXY_PORT + 1`, currently 34976). dnsmasq is NOT disabled -- it stays running and handles port 53 via DHCP. mitmdump's addon then enforces the same host allowlist for DNS (hostname match only, no path prefixes). This design avoids privileged ports and capabilities entirely. The `raw.dnsmasq` config is: `no-resolv\nno-poll\ncache-size=0\nserver=<gateway_ip>#<dns_port>\n`.

**Config resolution**: CLI arg > profile config > inherited parent profiles > default config > hardcoded default. Applied consistently in both `run()` functions -- don't bypass it.

There are three config-loading entry points, all on `Config`:
- `Config::load_default()` -- loads `~/.config/celily/config.toml` (the file is required; must contain `distro`).
- `Config::load_with_profile(name)` -- loads default, then merges `~/.config/celily/profiles/{name}.toml` over it.
- `Config::load_for_dir(cwd)` -- loads default, then reads `~/.config/celily/profiles.toml` (a `[profiles]` map of directory to profile name), canonicalizes every key, and does longest-prefix matching against `cwd`. The best match's profile is merged over the default. If the file is missing or no entry matches, returns the default config unchanged.

**Config file permissions**: the config directory (`~/.config/celily`), profiles directory (`~/.config/celily/profiles`), and individual config/profile files are all validated on load -- must be owned by the current user and have no group/other permissions set (`mode & 0o0077 != 0` -> rejection). `validate_config_dirs()` checks the directories; `validate_node_permissions()` checks individual files. This is a security boundary; config is never bind-mounted into containers, so a compromised tool can't modify it. That permission audit is the verification.

**Profile merging semantics** (in `Config::merge`):

- Scalar fields: profile wins if present, otherwise default.
- Lists (`mounts`, `allowed_dirs`, `allowed_files`, `pass_env`): concatenated (default first, profile appended). The `allow` sub-list inside `[network]` is also concatenated.
- Maps (`env`): merged; profile keys override default keys for the same name.
- Nested tables (`backend`, `limits`, `network`, `worktree`): merged field-by-field with scalar override rules.

If you add a new config field, add the merge logic in the appropriate sub-module's `merge()` function or it will silently never take effect from profiles.

**Boolean fields that default to true**: Use `Option<bool>` with a custom
serde default (e.g. `#[serde(default = "default_dns")]`) and merge with
`profile_field.or(default_field)`. This lets a profile explicitly set
`false` to disable something that defaults on. A plain `bool` with
`#[serde(default)]` can't distinguish "not set" from "set to false",
so `default || profile` would block profiles from turning the feature off.
See `NetworkConfig.dns` for the pattern.

**Config validation flow**: There is no automatic per-file or post-merge validation. If you add a validation rule, call it in `Config::load_file` (for per-file checks) or in `merge_with_profile` (for cross-file checks after merging).

**Mounts**: tilde expansion (`expand_host_tilde` / `expand_container_tilde`) happens before `validate_mount_source` is called, inside `resolve_context`. `validate_mount_source` canonicalizes the path, checks it against the forbidden list and allowlists (from `Config`), and returns the canonical form -- the original path in the `Mount` struct is replaced with the canonical one.

**Mount validation security model** (`validate_mount_source` in `celily/src/validate.rs`):

1. At least one of `allowed_dirs` or `allowed_files` must be non-empty.
2. Forbidden paths are checked first -- any match (exact or subtree, depending on variant) is a hard rejection.
3. The canonical path must be under `$HOME` (using `is_under_or_eq`).
4. The path must be an exact match in `allowed_dirs` (for directories) or `allowed_files` (for files). Subtrees are not implicitly allowed. Symlinks, sockets, and special files are rejected.

Forbidden path types:
- **Exact match only** (children are mountable if individually allowed): `$HOME`, `~/.config`, `~/.local`, `~/.local/share`
- **Subtree blocked** (everything underneath blocked): `~/.config/celily`, `~/.ssh`, `~/.gnupg`, `~/.local/share/keyrings`

If you add a new forbidden path, decide whether it should be `Forbidden::exact` or `Forbidden::under`, and update both `resolve_context` (where the list is built) and the man pages.

**Shared library changes**: if you add a public type to `celily-lib`, re-export it from `lib.rs`. Keep each concern in its own module -- don't grow `lib.rs` itself.

## User-facing docs

Man pages are written in **scdoc**(1) (`.scd` files under `man/`).
The generated roff files are written to `build/man/` and gitignored.
When editing docs:

1. Edit the `.scd` file
2. Run `make man` to regenerate the roff
3. Commit the `.scd` file (generated roff is not tracked)

The README is a user-facing overview — keep it current when adding
major features or changing the security model.

When changing CLI flags in `celily/src/cli_def.rs`,
the config schema in `celily/src/config/` (or any of its sub-modules),
or documented runtime behaviour, update the corresponding `.scd` file
and regenerate. gzip the roff into `man1/` and `man5/` under the manpath
when installing.

**scdoc constraints to watch for:**
- Only two heading levels (`#` and `##`); use bold text for sub-sub-headings
- No nested inline formatting (`*_nope_*`); close one before opening another
- Bullet continuations must be single-line; multi-line breaks the parser
- Indented lines starting with `-` are parsed as list items; escape with `\-`
  if you need a literal dash at the start of an indented line

## Config file

`$XDG_CONFIG_HOME/celily/config.toml` (typically `~/.config/celily/config.toml`). The file is required and must contain at least `distro`. The full schema is documented in `celily-config.5`.

Notable fields (all at top level; no `[common]` / `[run]` nesting):
- `[backend]` -- backend selection (kind: `"lxd"` or `"incus"`, project, pool). Default backend is `"lxd"`
- `distro` -- **required**; which distribution the image is based on (currently only `"arch"`)
- `image` -- LXD image alias or fingerprint to launch (default: `"celily"`)
- `vm` -- run as a VM instead of a container (default: `false`)
- `user` -- non-root username inside the container (default: `"dev"`)
- `container_uid` -- UID assigned to `user` inside the container (default: `1000`)
- `container_gid` -- GID assigned to `user`'s primary group inside the container (default: `1000`)
- `security_nesting` -- enable `security.nesting` on the container; required for snapd, Docker, etc. (default: `false`). Ignored on VMs
- `secure_boot` -- disable UEFI secure boot for VMs (default: `true`). Ignored on containers
- `mount_project` -- whether to bind-mount the current directory into the container (default: not mounted; worktree mode always mounts)
- `project_readonly` -- whether the project mount is read-only (default: `false`, forced `true` in worktree mode)
- `project_target` -- target path inside the container for the project bind-mount (default: `~/project`)
- `[[mounts]]` -- bind mounts added to every container (source/target paths are tilde-expanded; `readonly` is optional, default false)
- `env` -- static environment variables; lowest priority
- `pass_env` -- host env var names to forward; overrides `env` for same key
- `secret_provider` -- name of the active secret provider (currently only `"rbw"`); used for `auth.secret` in network allow rules. If absent, auth secrets are not resolved and will error if referenced
- `allowed_dirs` / `allowed_files` -- mandatory (at least one non-empty); exact-match allowlists for mount sources
- `[limits]` -- cpu, memory, disk, processes, disk_priority, memory_enforce, network_ingress, network_egress (container-only limits warn on VMs but are passed through)
- `[network]` -- network isolation config (allow, dns); `allow` rules use a `type` discriminator (`"http"` or `"tcp"`). HTTP rules are enforced by mitmdump; TCP rules by the bridge egress ACL. See `celily-config.5` for full schema.
- `[worktree]` -- worktree mode configuration (branch, auto_commit, user_name, user_email)
- `pre_run` -- inline script run before the main command
- `notifications` -- whether to bind-mount the notification proxy socket (default: true)

### Profiles and profiles.toml

Named profiles live at `~/.config/celily/profiles/{name}.toml` and have the same schema as the main config. They are merged over the default config -- only the fields they set override.

`~/.config/celily/profiles.toml` maps directories to profile names:

```toml
[profiles]
"/home/user/project" = "work"
"/home/user/project/oss" = "oss"
```

At launch (`Config::load_for_dir`), each key is canonicalized and the longest-prefix match against the current working directory determines the profile. If no entry matches, only the default config is used. `--profile` / `--no-profile` CLI flags bypass auto-detection entirely.

## Command resolution

If CLI args were given after `--`, they are used as-is.
Otherwise falls back to a POSIX login shell (`sh -l`).
The logic is inlined in `main.rs`.

## Environment variable priority

Lowest to highest: `env` -> `pass_env` -> `--pass-env` -> `--env`.

Forwarded vars (`pass_env` / `--pass-env`) override static `env` entries with the same key. CLI `--env` is the highest priority.

**Proxy env vars** (`HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`) are injected at the highest priority -- they override `--env`, `--pass-env`, and `env`/`pass_env` entries with the same key. This is intentional: if the agent could override the proxy it could bypass isolation.

## Secrets handling

`celily-lib/src/secrets/mod.rs` + `celily-lib/src/secrets/rbw.rs`:

1. `Providers::Rbw.resolve()` (or `match provider_name { "rbw" => Providers::Rbw, ... }.resolve()`) creates a boxed `SecretProvider` trait object.
2. `check_available()` verifies the provider is usable (e.g. `rbw unlocked` for Bitwarden).
3. Secrets are resolved on-demand from `auth.secret` entries in `network.allow` rules during `NetworkIsolation::setup()`. Each unique secret name is resolved once via `resolve()`, then the resolved values are embedded into the config JSON sent to mitmdump over the Unix socket.
4. Secrets never appear as environment variables in the container -- they are routed to mitmdump only, which injects them as HTTP headers on matching requests.

If you add a new provider, implement the `SecretProvider` trait and add a variant to the `Providers` enum.

## Things that look odd but are intentional

- `lxc init` + `lxc start` instead of `lxc launch`: the split allows for post-init configuration before first boot if needed in the future.
- `InstanceGuard::Drop` logging errors but not panicking: the container might already be gone (e.g. ephemeral instance auto-deleted on stop), so failures here are expected and non-fatal.
- Images are built externally (there is no image builder in this repo). The image must include the distro's CA certificate trust infrastructure for network isolation to work. See `celily-config.5` IMAGE REQUIREMENTS.
- `vm` serialized as `vm = true` in TOML (not `kind = "vm"`): `InstanceKind` deserializes from a bool via a custom `Deserialize` impl. The `--vm` CLI flag maps to the same behaviour.
- `raw_idmap` always emits `uid {host_uid} {container_uid}` and `gid {host_gid} {container_gid}` as two separate lines joined with `\n`. No `both` line, no conditionals.
- `bridge_name()` produces a max-15-char bridge name from the image prefix and UUID: `<prefix6>-<uuid8>`. Linux bridge names are capped at 15 characters.
- `Instance<Running>` field order: `guard` (InstanceGuard) is declared before `isolation` (NetworkIsolation) so Rust's drop order guarantees the instance is deleted before the isolation bridge is torn down. Reordering these fields will cause `lxc delete` failures.

## Read-tool gotchas

Never edit the generated roff files under `build/man/` directly -- they are
regenerated from `.scd` sources by `make man`. Always edit the `.scd`
file instead.
