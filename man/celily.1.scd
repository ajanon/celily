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
	underneath them and every directory containing them, regardless of the
	allowlists.

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
	_NAME_ may contain ASCII letters, digits, *.*, *\_* and *-*, and must
	not start with *.* or *-*.

*--worktree-user-name* _NAME_
	Override git user.name for commits made in worktree mode.

*--worktree-user-email* _EMAIL_
	Override git user.email for commits made in worktree mode.

*--mount-project*
	Bind-mount the current working directory (project directory) into the
	container. The default is to not mount it -- this is an opt-in flag.
	Ignored in worktree mode, which always mounts the project directory.

*--no-mount-project*
	Explicitly disable the project directory mount (overrides config).
	Conflicts with *--mount-project* and *--worktree*.

*--project-readwrite*
	Mount the project directory read-write, *.git* included. The default
	is read-only. Conflicts with *--worktree*. See *Read-write mounts*.

*--no-project-readwrite*
	Mount the project directory read-only, overriding _project_readwrite_
	in the config. Conflicts with *--project-readwrite*.

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

When *--worktree* _NAME_ is passed, *celily* mounts the project directory
read-only, *.git* included (*--project-readwrite* is rejected and
_project_readwrite_ ignored in worktree mode), and clones it inside the container into _~/NAME_. The clone
borrows the project's objects read-only (*git clone --shared*), so nothing
is copied. New commits stay in the clone until you fetch them (see
*Fetching commits*): the sandbox cannot write to the host repository.

The branch is *celily/*_NAME_. If it exists in the host repository, the
clone starts from it; otherwise it is created from the host checkout's
HEAD.
Inside the sandbox, *git fetch origin* picks up new host commits.

The commit author and committer are taken from _[worktree].user_name_
and _user_email_, falling back to the host project's git
config (or the *--worktree-user-name* / *--worktree-user-email*
CLI overrides). Uncommitted changes are lost at teardown.

The clone depends on the host repository's objects. Do not prune them
(e.g. *git gc --prune=now* after deleting branches) while a run is active.

Worktree mode requires:
- A git repository with at least one commit.
- *.git* to be a directory (not a symlink or submodule).
- No submodules (_.gitmodules_ must not exist).
- git's *ext::* transport allowed as *user* (see below).

## Fetching commits

For the duration of the run, *celily* adds a git remote to the host
repository, named after the branch (e.g. *celily/feature-x*). Fetch the
sandbox's commits whenever you want to review them, from the host:

```
$ git fetch celily/feature-x
```

The remote's URL runs *git upload-pack* inside the instance through
*lxc exec* (or *incus exec*). Only refs and objects cross the boundary: the sandbox
repository's config and hooks only ever run inside the sandbox, and the
host's *git fetch* hash-checks every object it receives. The remote is
fetch-only and updates exactly one ref, the host branch of the same name,
forcibly (the sandbox may rewrite its history). Tags are not fetched,
and the remote never prunes, even with *fetch.prune* or *fetch.pruneTags*
set. It is skipped by *git fetch --all*.

Nothing is fetched automatically. The remote is removed when *celily*
exits; commits that were not fetched by then are lost with the instance.
If *celily* is killed, its cleanup does not run: the remote remains, and
so does the instance, still running (see *Cleanup* below). Fetching from
the remote works for as long as the instance does. The next run with the
same name replaces the remote. A remote of that name
whose URL is not an *ext::* one is never replaced: the run fails instead.

*git fetch* refuses to update a branch that is checked out in one of the
host repository's worktrees, including stale entries (see
*git worktree list*). Run *git worktree prune* to drop entries whose
directory no longer exists.

## Allowing the ext transport

The remote uses git's *ext::* transport, which runs a local command as
the other end of a fetch. git refuses it by default, and *celily* will
not change that for you: worktree mode fails until the repository allows
it. Allowing it for this repository only, for commands you run yourself:

```
$ git config protocol.ext.allow user
```

With *user*, an *ext::* URL runs its command when you fetch, clone or push
with it in this repository, and git still refuses it when it fetches on
its own behalf (e.g. submodules). The trade-off: an *ext::* URL in this
repository's config, or one you paste, runs a command, like a pasted
shell command would. Whoever can write this repository's config can
already make git run commands (hooks, *core.fsmonitor*,
*core.sshCommand*), so this adds little on top.

*celily* refuses *always*: the sandbox could commit a _.gitmodules_ with
an *ext::* URL, and *git submodule update --init* would run its command
on the host. A submodule clone does not read this repository's config,
so a local *user* does not override an *always* from the system or global
config, *git -c*, or a *GIT_ALLOW_PROTOCOL* listing *ext*. The check runs
at launch only.

*celily* also warns when *fetch.fsckObjects* (or *transfer.fsckObjects*)
is off. Objects are hash-checked either way; fsck additionally rejects
malformed objects and dangerous paths. Recommended:

```
$ git config fetch.fsckObjects true
```

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

	A source containing one of the paths marked "anything underneath" is
	forbidden too. This matters when they are symlinks: with
	*~/.ssh* -> *~/dotfiles/ssh*, *~/dotfiles* cannot be mounted.

*3. Exact-match allowlist*
	The source must be listed exactly (not as a subtree) in
	_allowed_dirs_ (for directories) or _allowed_files_ (for files) in
	the config. Subtrees of allowed directories are not implicitly allowed.

*4. Symlinks and special files rejected*
	A source, or an allowlist entry, that goes through a symlink under
	*$HOME* is rejected: a sandbox with write access to its directory
	could swap it for a symlink to somewhere else, and the next run would
	follow it. Sockets and other special files are rejected too. Only
	regular files and directories can be mounted.

These rules exist because the tools run inside the container (e.g. coding
agents) may execute arbitrary commands. The allowlist is therefore the
boundary between "accessible to the container" and "not accessible".
If a directory is not listed, it cannot be mounted -- there is no fallback,
no implicit allowance, no exception for subdirectories.

All configuration lives in *$XDG_CONFIG_HOME/celily/*, which is always
blocked from bind-mounts. There is no per-project config file and no
environment variable that can redirect the config path.

## What a mount exposes

A mount exposes the whole directory, read-only or not: untracked and
ignored files (*.env*, local credentials, build output) and, in a
repository, all of *.git*: remote URLs (some embed credentials), stashes,
the reflog, and every local branch. Worktree mode mounts the project the
same way. Keep secrets out of mounted directories.

## Read-write mounts

Mounts are read-only unless set otherwise. In a read-write repository,
the sandbox can plant hooks and config in *.git* (e.g. *core.fsmonitor*)
that git runs on the host, and build scripts in the working tree. Mount a
repository read-write only if you would run the sandbox's code on the
host yourself; use worktree mode otherwise. *celily* warns at startup
about every read-write mount, the project directory included.

## Network security

Every *celily* invocation creates a per-instance LXD/Incus bridge with
an egress ACL that blocks all TCP traffic except connections to a
per-instance proxy running inside the *celily* process. The proxy
enforces an allowlist of permitted hosts and optional path prefixes,
and injects authentication headers where configured (see _auth_ in
*celily-config*(5)). The proxy listens on the host's address on the
bridge, and only serves the instance: requests from anywhere else, other
containers included, get a 403.
*HTTP_PROXY*, *HTTPS_PROXY*, *NODE_USE_ENV_PROXY* and *NODE_USE_SYSTEM_CA*
are forcibly injected so that proxy-aware tools work transparently. They
are a convenience, not the boundary: a process can drop them. Tools that
ignore them reach nothing -- the bridge ACL rejects every TCP connection
but those to the proxy and to TCP rules.

Instances are created without any LXD/Incus profile: a profile could
add a network interface outside the isolation bridge, extra devices, or
raw configuration. Every device of the instance comes from *celily*.

## Cleanup

The instance, its bridge, and its ACL are deleted when *celily* exits.
*celily* does not handle signals: if it is killed (closing its terminal
is enough), the instance keeps running, with its mounts and the TCP
rules of its ACL; only the proxy, which runs inside *celily*, is gone.
Delete it with *lxc delete --force* _NAME_, and the bridge and ACL with
*lxc network delete* and *lxc network acl delete*.

## Disk exhaustion

The root disk is constrained by *--disk*, but bind-mounts (the project
directory, *--mount*, _[[mounts]]_) do not count against the container's
quota. A tool that writes large files to a read-write mount can exhaust
the host's free space. Keep mounts read-only (the default) where possible.

# FILES

*$XDG_CONFIG_HOME/celily/config.toml*
	Global configuration file.

*$XDG_CONFIG_HOME/celily/profiles.toml*
	Optional directory-to-profile mapping.

*$XDG_CONFIG_HOME/celily/profiles/*_NAME_*.toml*
	Profile configuration files (same schema as config.toml).

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

Review a worktree's commits from the host, while it runs:
```
$ git fetch celily/bugfix
$ git log main..celily/bugfix
```

# SEE ALSO

*celily-config*(5), *lxc*(1)
