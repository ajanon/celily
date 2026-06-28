celily(1) "" "celily"

# NAME

celily - launch an ephemeral LXD/Incus container for running commands

# SYNOPSIS

*celily* [_OPTIONS_] [*--* _ARGS_]

# DESCRIPTION

*celily* launches an ephemeral LXD/Incus container (or VM) from a pre-built image,
and either drops into a shell or runs a configured command with the given arguments.
The project directory (current working directory) is *not* mounted by default;
use *--mount-project* or set _mount_project = true_ to bind-mount it.
The default mount target is *~/project* (relative to the container home);
use *--project-target* or _project_target_ to change it.
The instance is deleted automatically on exit unless
*--keep-instance* is used.

Images are built externally (e.g. with *lxc publish*) and
pointed at via the _image_ config setting. Configuration is read
from *celily-config*(5).

# OPTIONS

*--image* _NAME_
	Image to launch. Default: from the _image_ config setting, then _celily_.

*--name* _NAME_
	Override the auto-generated instance name.

*--keep-instance*
	Keep the instance after exit. Implies non-ephemeral.

*--vm*
	Run as a VM instead of a container. Default: from _vm_, then false.

*--mount* _SOURCE_*:*_TARGET_[*:readwrite*|*:readonly*]
	Bind-mount a host directory into the container. Repeatable. Source paths
	starting with *~* expand to the host home directory; target paths starting
	with *~* expand to the container home directory.
	The optional third field sets the access mode: *readwrite* or *readonly*.
	Read-only is the default when omitted.
	*Every mount source must be under the current user's $HOME directory
	and must be listed exactly in* _allowed_dirs_ *or* _allowed_files_ *(see*
	*celily-config*(5)*).* *$HOME*, *$XDG_CONFIG_HOME*, *~/.local*,
	and *~/.local/share* are permanently forbidden (exact match only -- children
	can be mounted). *$XDG_CONFIG_HOME/celily*, *~/.ssh*, *~/.gnupg*, and
	*~/.local/share/keyrings* are permanently forbidden along with everything
	underneath them regardless of the allowlists.

*--cpu* _N_
	Number of vCPUs. Default: from _limits.cpu_, then 2.

*--memory* _SIZE_
	Memory limit (e.g. *4GiB*). Default: from _limits.memory_, then *4GiB*.

*--disk* _SIZE_
	Root disk size (e.g. *4GiB*). Default: from _limits.disk_, then *4GiB*.

*--processes* _N_
	Maximum number of processes; *-1* for unlimited. Default: from
	_limits.processes_, then 1024.

	Setting *-1* (unlimited) removes fork-bomb protection.

*--user* _NAME_
	User to exec as inside the container. Default: from _user_, then _dev_.

*--root*
	Exec as root (uid 0) instead of the configured user.

*--container-uid* _UID_
	UID of the non-root user inside the container. Used for idmap configuration.
	Default: from _container_uid_, then 1000.

*--container-gid* _GID_
	GID of the non-root user's primary group inside the container. Used for
	idmap configuration. Default: from _container_gid_, then 1000.

*--env* _KEY_*=*_VALUE_
	Set an environment variable inside the container. Repeatable. Highest priority;
	overrides forwarded variables and static config entries.

*--pass-env* _NAME_
	Forward a host environment variable into the container. Repeatable.
	Overrides static _env_ entries for the same key.

*--project-target* _PATH_
	Target path for the project bind-mount inside the container. Tilde
	expands to the container home directory. Default: *~/project*.

*--profile* _NAME_
	Load the named profile from *$XDG_CONFIG_HOME/celily/profiles/*_NAME_*.toml*
	and merge it over the default config. If the profile has an
	_[inherit]_ entry in _profiles.toml_, the parent is merged
	first. Skips auto-detection.

*--no-profile*
	Skip profile auto-detection; use only the default config. Mutually
	exclusive with *--profile*.

*-w*, *--worktree* _NAME_
	Enable worktree mode with the given name. See *WORKTREE MODE* below.

*--no-auto-commit*
	Disable the safety-net auto-commit in worktree mode.

*--worktree-user-name* _NAME_
	Override git user.name for worktree auto-commits.

*--worktree-user-email* _EMAIL_
	Override git user.email for worktree auto-commits.

*--mount-project*
	Bind-mount the current working directory (project directory) into the
	container. The default is to not mount it -- this is an opt-in flag.
	Ignored in worktree mode, which always mounts the project directory.

*--no-mount-project*
	Explicitly disable the project directory mount (overrides config).
	Conflicts with *--mount-project* and *--worktree*.

*--project-readonly*
	Mount the project directory read-only.

*--no-project-readonly*
	Explicitly disable the read-only project mount (overrides config).
	Conflicts with *--project-readonly* and *--worktree*.

## Positional arguments

Arguments after *--* are passed to the command running inside the
container. If no arguments are given, a POSIX login shell (*sh -l*)
is started.

# PROFILE AUTO-DETECTION

By default, *celily* reads *$XDG_CONFIG_HOME/celily/profiles.toml* and
matches the current working directory against its entries using longest-prefix
matching. If multiple entries match, the one with the longest path wins.
The matched profile is loaded from *$XDG_CONFIG_HOME/celily/profiles/*_NAME_*.toml*
and merged over *$XDG_CONFIG_HOME/celily/config.toml*.
If the profile has an _[inherit]_ entry, the parent profile is merged
first (see *[inherit] SECTION* in *celily-config*(5)).

Use *--profile* _NAME_ to override auto-detection, or
*--no-profile* to use only the default config.

# USER

Commands run as the configured user (_dev_, uid 1000 by default) inside
the container. Use *--root* to exec as root instead, or
*--user* _NAME_ to specify a different username.
The project directory is mounted at the target configured via
*--project-target* or _project_target_
(default: *~/project*, relative to the container home).

# WORKTREE MODE

When *--worktree* _NAME_ is passed, *celily* creates a git
worktree inside the container instead of bind-mounting the project directory
directly. The project directory is always mounted read-only
(*--no-project-readonly* is ignored in worktree mode),
with *.git* overlaid read-write so git operations work.

The worktree is created in the container home directory under a name
unique to the instance, preventing collisions between concurrent
*celily* runs. The branch name is derived from
_[worktree].branch_ (default: *celily/{name}*), with
_{name}_ replaced by the value passed to *--worktree*.

Before teardown, uncommitted changes are automatically committed as a
safety net (unless *--no-auto-commit* is passed). The commit
author and committer are taken from _[worktree].user_name_
and _user_email_, falling back to the host project's git
config (or the *--worktree-user-name* / *--worktree-user-email*
CLI overrides). The worktree metadata is cleaned up so the host
repository's *.git* directory remains untouched.

Worktree mode requires:
- A git repository with at least one commit.
- *.git* to be a directory (not a symlink or submodule).
- No submodules (_.gitmodules_ must not exist).

# IDMAPS

For containers (not VMs), *celily* configures _raw.idmap_ with
_uid_ and _gid_ lines mapping the host user's uid and gid to the
container uid and gid, so bind-mounted project files have the correct
ownership.

# MOUNT SECURITY

Every bind-mount source is validated against a strict set of rules before the
container starts:

*1. The source must be under the current user's $HOME directory.*

*2. The following paths are permanently forbidden:*
	- *$HOME* -- Mounting the entire home directory would expose every file the user owns.
	- *$XDG_CONFIG_HOME* -- The config directory root is too broad to mount safely.
	- *$XDG_CONFIG_HOME/celily* and anything underneath -- The celily configuration subtree must never be writable by agents.
	- *~/.ssh* and anything underneath -- Contains private keys and sensitive SSH configuration.
	- *~/.gnupg* and anything underneath -- Contains GPG private keys.
	- *~/.local* -- The directory itself is blocked. Children (e.g. *~/.local/bin*) can be mounted if individually listed in _allowed_dirs_.
	- *~/.local/share* -- The directory itself is blocked. Children other than _keyrings/_ can be mounted if individually listed in _allowed_dirs_.
	- *~/.local/share/keyrings* and anything underneath -- Contains GNOME Keyring / user secret storage.

*3. Exact-match allowlist*
	The source must be listed exactly (not as a subtree) in
	_allowed_dirs_ (for directories) or _allowed_files_ (for files) in
	the config. Subtrees of allowed directories are not implicitly allowed.

*4. Special files rejected*
	Symlinks, sockets, and other special files are rejected. Only regular
	files and directories can be mounted.

These rules exist because the tools run inside the container (e.g. coding
agents) may execute arbitrary commands. The allowlist is therefore the
boundary between "accessible to the container" and "not accessible".
If a directory is not listed, it cannot be mounted -- there is no fallback,
no implicit allowance, no exception for subdirectories.

All configuration lives in *$XDG_CONFIG_HOME/celily/*, which is always
blocked from bind-mounts. There is no per-project config file and no
environment variable that can redirect the config path.

## Network security

Every *celily* invocation creates a per-instance LXD/Incus bridge with
an egress ACL that blocks all TCP traffic except connections to a
per-instance mitmproxy. The mitmproxy enforces an allowlist of
permitted hosts and optional path prefixes, and injects authentication
headers where configured (see _auth_ in *celily-config*(5)).
*HTTP_PROXY* and *HTTPS_PROXY* are forcibly injected so that
proxy-aware tools work transparently. Tools that ignore the proxy
environment variables simply cannot reach the internet on ports 80 or
443 -- the bridge ACL drops their packets.

If more granular network containment is needed, configure it via
LXD/Incus profiles or a custom bridge at the host level.

mitmproxy logs are written to
*$XDG_STATE_HOME/celily/logs/mitmdump_*_NAME_*.log*
(typically *$XDG_STATE_HOME/celily/logs/*).
The path is printed at instance start. Logs are removed on clean exit
and preserved when mitmdump exits with an error.

## Disk exhaustion

The root disk is constrained by *--disk*, but the project directory
bind-mount (when enabled via *--mount-project* or
_mount_project_) does not count against the container's quota.
A tool that writes large files to a mounted directory can exhaust the
host's free space. If this is a concern, set the project bind-mount to
read-only via *--project-readonly*, or configure
_project_readonly_ in the config. Keep other mounts read-only where
possible.

# FILES

*$XDG_CONFIG_HOME/celily/config.toml*
	Global configuration file.

*$XDG_CONFIG_HOME/celily/profiles.toml*
	Optional directory-to-profile mapping.

*$XDG_CONFIG_HOME/celily/profiles/*_NAME_*.toml*
	Profile configuration files (same schema as config.toml).

*$XDG_STATE_HOME/celily/logs/mitmdump_*_NAME_*.log*
	mitmproxy logs (removed on clean exit, kept on error).

# EXAMPLES

Open a shell in an ephemeral container:
```
$ celily
```

Run a specific command:
```
$ celily -- cargo build
```

Use a named profile:
```
$ celily --profile rust -- cargo build
```

Skip profile auto-detection:
```
$ celily --no-profile -- cargo build
```

Keep the instance alive after exit:
```
$ celily --keep-instance -- bash -c 'make test'
```

Forward *EDITOR* and mount an extra directory:
```
$ celily --pass-env EDITOR --mount ~/secrets:/secrets:readwrite -- just build
```

Open a shell in a git worktree:
```
$ celily -w feature-x -- bash
```

Run a command in a worktree:
```
$ celily -w bugfix -- cargo test
```

# SEE ALSO

*celily-config*(5), *lxc*(1)
