celily-config(5) "" "celily"

# NAME

celily-config - celily configuration file format

# DESCRIPTION

*celily*(1) is configured through TOML files.
All configuration files live under *$XDG_CONFIG_HOME/celily/*, which is always
blocked from bind-mounts (see *MOUNT SECURITY* in *celily*(1)).
There is no per-project configuration file.

# FILE LAYOUT

*$XDG_CONFIG_HOME/celily/config.toml*
	Global configuration file providing default values for all settings.
	The config file is required.

*$XDG_CONFIG_HOME/celily/profiles.toml*
	Optional mapping of directory paths to profile names. Each entry maps a
	path (with optional tilde expansion) to a profile name. When *celily*
	runs, it matches the current working directory against these entries
	(longest prefix wins) and loads the corresponding profile. Missing file
	or missing entries mean no profile is applied.

	Example:
```
[profiles]
"~/Projects/work" = "work"
"~/Projects/opensource" = "oss"
```

	An optional _[inherit]_ section in the same file declares
	profile inheritance (see *[inherit] SECTION* below).

*$XDG_CONFIG_HOME/celily/profiles/*_NAME_*.toml*
	Profile configuration files. Each has the same schema as _config.toml_
	(see below). The profile is merged _over_ the default config: profile
	values override defaults, lists are extended, and maps are merged.
	Missing profile files referenced from _profiles.toml_ are a hard error.

# PRIORITY

Configuration is resolved in this order (lower overrides higher):

*1. Hardcoded defaults*

*2. Global config* (_config.toml_)

*3. Parent profiles* (if _[inherit]_ declares a chain), from root parent to direct parent

*4. Profile config* (_profiles/NAME.toml_)

*5. CLI arguments*

# SECURITY MODEL

All configuration lives in *$XDG_CONFIG_HOME/celily/*, a directory that is
never bind-mounted into containers or VMs. A compromised tool running
inside the container cannot read or modify configuration files. There
are no per-project config files, and there is no environment variable
that can redirect the configuration path.

Because the entire configuration tree is trusted, every field can be
merged from profiles without restriction. Lists are additive, maps are
merged (profile keys take precedence), and scalars are overridden.

# MERGING RULES

When a profile is loaded, its values are merged over the default config
with the following rules:

*Scalars*
	Profile value wins if present; otherwise the default value is used.

*Lists*
	Lists from the default and profile are concatenated (default first,
	profile appended). This applies to _mounts_, _proxy_,
	_network.allow_, _allowed_dirs_, _allowed_files_, and _pass_env_.

*Maps*
	Maps from the default and profile are merged; profile entries with the
	same key override default entries. This applies to _env_.

*Nested tables*
	Fields within _limits_ and _worktree_ are merged field-by-field
	using the same scalar override rules.

# [inherit] SECTION

The optional _[inherit]_ section in _profiles.toml_ declares
profile inheritance. Each entry maps a profile name to its parent.
Transitive chains are supported: if _work_ inherits from
_base_, and _base_ inherits from _common_, then
_work_ receives settings from both _common_ and _base_
(with _work_ taking highest priority).

Example:

```
[inherit]
common = "minimal"
base = "common"
work = "base"
```

When a profile has an _[inherit]_ entry, the entire inheritance
chain is walked and each profile is merged from root to leaf. For the
example above the merge order is:

	default _config.toml_ -> _minimal_ -> _common_ -> _base_ -> _work_

*Errors*
	A profile cannot inherit from itself. A circular chain (e.g. _a_
	inherits _b_ which inherits _a_) produces a hard error. Every
	profile in the chain must exist on disk and have valid permissions.

*Resolution*
	Both *--profile* and profile auto-detection (via the
	_[profiles]_ directory mapping) resolve inheritance. The
	_[inherit]_ section is consulted whenever a profile is loaded,
	regardless of how it was selected.

# TOP-LEVEL KEYS

*distro* = _string_
	*Required.* Distribution the image is based on. Supported distros:
	- *arch* (Arch Linux)

	Example: _distro = "arch"_

*image* = _string_
	LXD/Incus image alias or fingerprint to launch. Default: _celily_.

*vm* = _bool_
	Run as a VM instead of a container. Default: false.

*user* = _string_
	Non-root user created inside the container. Default: _dev_.

*container_uid* = _integer_
	UID assigned to the non-root user; used for idmaps. Default: 1000.

*container_gid* = _integer_
	GID assigned to the non-root user's primary group; used for idmaps.
	Default: 1000.

*security_nesting* = _bool_
	Enable LXD/Incus _security.nesting_ on the container. Required for
	snapd, Docker, and other tools that perform filesystem mounts inside
	the container. Ignored on VMs. Default: false.

*secure_boot* = _bool_
	Disable UEFI secure boot for VMs. Set to false when the image does not
	support secure boot (e.g. Arch Linux cloud images). Ignored on
	containers. Default: true (LXD/Incus default; leaves _boot.mode_ unchanged).

# IMAGE REQUIREMENTS

*celily* does not build images. You are expected to provide your
own LXD/Incus image (built with *lxc publish*, a bash script, or any
automation tool) and point _image_ at its alias or
fingerprint.

For network isolation to work, the image must include the
distribution's CA certificate trust infrastructure so that
*celily* can inject its per-instance MITM CA certificate before
first boot.

*Arch Linux*

	Packages:
	- *ca-certificates*
	- *ca-certificates-utils*

*Debian / Ubuntu*

	Package:
	- *ca-certificates*

If the CA infrastructure is missing, *celily* fails at launch
when it cannot inject the MITM certificate into the image.
This is a hard requirement; there is no way to disable
network isolation.

# RUNTIME SETTINGS

Settings for *celily*(1). All fields are optional.

*pre_run* = _string_
	Inline script run as the container user before the command being run
	(or the shell, if no arguments are given). The script is written to a temp file
	and executed, so any shebang is honored -- *#!/usr/bin/env python3*
	works as well as plain shell. The same environment variables as the main
	command are set (including proxy vars, _[env]_,
	_[pass_env]_, and *HOME*).

	Use for per-run setup that needs to happen before the command executes:
	refreshing credentials from a host-mounted socket, running a health
	check, pre-warming a cache. An empty or whitespace-only value is
	treated as not set. The exit code of the script determines whether
	the run proceeds or aborts.

	Example:
```
pre_run = '''
curl -s --unix-socket /tmp/agent.sock http://localhost/refresh
'''
```

*project_target* = _string_
	Target path inside the container for the project bind-mount. A leading
	*~* expands to the container home directory. Absolute paths and paths
	outside the container home are allowed -- the container is ephemeral. This
	also sets the working directory for the command run inside the container.
	Default: *~/project*.

	Example: _project_target = "~/src"_ mounts the host CWD at
	*/home/dev/src* and sets the working directory to the same path.

*mount_project* = _bool_
	When *true*, the current working directory (project directory)
	is bind-mounted into the container. When *None* (not set),
	the project directory is not mounted unless worktree mode is active.
	Defaults to not mounted. Worktree mode (*--worktree*) always
	mounts the project directory regardless of this setting.
	Set to *false* to explicitly disable (though worktree mode still
	overrides).

*project_readonly* = _bool_
	When *true*, the project directory is mounted read-only.
	When *None* (not set), the default depends on worktree mode:
	read-only when worktree mode is active, read-write otherwise.
	Defaults to *false* when not in worktree mode.
	When worktree mode is active (*--worktree*), the project is
	always mounted read-only regardless of this setting.

*allowed_dirs* = _[paths]_
	*Required.* Directory paths that may be bind-mounted. A leading *~* expands to
	the host home directory. Every mount source (including the current working
	directory) must be under the current user's *$HOME*, and must be an
	exact match for an entry in this list. Subtree matching is intentionally
	rejected -- only explicitly listed directories can be mounted.

*allowed_files* = _[paths]_
	*Required.* File paths that may be bind-mounted. A leading *~* expands to the
	host home directory. Same security model as _allowed_dirs_: exact
	match only, must be under *$HOME*.

*pass_env* = _[strings]_
	Names of host environment variables to forward into the container.
	Overrides static _env_ entries for the same key.

*env* = _{ key = "value" }_
	Static environment variables set for every command run inside the container.
	See *[env] SECTION* below.

*secret_provider* = _string_
	Name of the active secret provider. Available providers:
	- *rbw* -- Bitwarden CLI (requires *rbw*(1) installed and vault unlocked).

	If absent, secrets are not resolved. When a network allow rule references
	an _auth.secret_ without a configured provider, *celily* exits
	with an error.

# [[proxy]] SECTION

Array of proxy devices that expose host Unix sockets inside the
instance. Each entry maps to an LXD/Incus *proxy* device (see
*lxc*(1) *device add proxy*).

*Forbidden paths:* Certain paths are permanently blocked regardless
of ownership: *~/.ssh/agent/*, and under *$XDG_RUNTIME_DIR*:
*gnupg/*, *keyring/*, *rbw/*, *gcr/*, *p11-kit/*,
*systemd/*, *podman/*, and *bus*. These directories and
sockets grant access to secrets, session control, or container
management and cannot be proxied.

*Boundary:* The socket path must be under *$HOME* or
*$XDG_RUNTIME_DIR* (falling back to */run/user/<uid>*).

*connect* = _string_ (required)
	Host-side socket path in LXD/Incus proxy format. Only *unix:*
	sockets are supported (e.g. *unix:/run/user/1000/sock*, no
	abstract sockets).

*listen* = _string_ (required)
	Container-side listen path in LXD/Incus proxy format (e.g.
	*unix:/run/sock*).

*bind* = _string_ (optional)
	Which side the proxy listens on, *"instance"*, or *"host"*.
	Default: *"instance"*.

*mode* = _string_ (optional)
	File mode for the socket inside the container (e.g. *"0644"*).
	Default: *"0600"*.

The proxy always runs as the current host user and owns the socket as the
container user.

Example (notifications via xdg-dbus-proxy):
```
[[proxy]]
connect = "unix:/run/user/1000/dbus-notifications.sock"
listen = "unix:/run/dbus-notifications.sock"
```

# [worktree] SECTION

Worktree mode configuration. Worktree mode is activated by passing
*--worktree* _NAME_ on the CLI (see *WORKTREE MODE*
in *celily*(1)). This section configures its behaviour.
All fields are optional.

*branch* = _string_
	Template for the worktree branch name. _{name}_ is replaced with
	the value passed to *--worktree*. Default: *celily/{name}*.

*auto_commit* = _bool_
	Automatically commit uncommitted changes before container teardown.
	Default: *true*.

*user_name* = _string_
	Git user.name for auto-commits. Falls back to the host project's
	git config when *None*.

*user_email* = _string_
	Git user.email for auto-commits. Falls back to the host project's
	git config when *None*.

# [backend] SECTION

Instance backend selection and LXD/Incus daemon configuration.

*kind* = _string_
	Which backend to use for managing instances and network isolation.
	Supported values:
	- *"lxd"* -- Use *lxc*(1) (LXD daemon). Default.
	- *"incus"* -- Use *incus*(1) (Incus daemon).

*project* = _string_
	LXD project name that all *lxc*(1) / *incus*(1) commands are
	scoped to (via *--project*). When absent, the daemon's current
	default project is used. The project is created automatically on first
	use with the necessary features enabled (_features.images_). Optional.

	Example: _project = "celily"_.

*pool* = _string_
	Name of the storage pool used for the instance's root disk device.
	Applies to both containers and VMs. The pool is set in the project's
	default profile via *lxc profile device add* during project
	setup. When a non-default disk size is configured, it overrides the
	profile's root device. Default: *"default"*.

	Example: _pool = "zfs"_.

# [limits] SECTION

Resource limits for *celily*(1). All fields are optional.

*cpu* = _integer_
	Number of vCPUs. Default: 2.

*memory* = _string_
	Memory limit (e.g. *4GiB*). Default: *4GiB*.

*disk* = _string_
	Root disk size (e.g. *4GiB*). Default: *4GiB*.

*processes* = _integer_
	Maximum number of processes; *-1* for unlimited. Default: 1024.
	Containers only. Ignored with a warning on VMs.

	Setting *-1* removes fork-bomb protection. A malicious tool inside
	the container can launch a DoS attack against other hosts (see
	*NETWORK SECURITY* in *celily*(1)).

*network_ingress* = _string_
	Incoming network bandwidth limit in bit/s (e.g. *100Mbit*).
	Supported suffixes: *bit*, *kbit*, *Mbit*, *Gbit*
	(decimal) and *Kibit*, *Mibit*, *Gibit* (binary).

*network_egress* = _string_
	Outgoing network bandwidth limit in bit/s. Same suffix support as
	_network_ingress_.

*disk_priority* = _integer_
	Disk I/O priority (0--10, 0 = lowest priority). Containers only. Ignored on VMs.

*memory_enforce* = _string_
	Memory enforcement (*soft* or *hard*). Containers only. Ignored on VMs.

# [[mounts]] SECTION

Array of bind mounts added to every container. Each entry has three fields.
*All mount sources must be under the current user's $HOME directory
and must match an entry (exact match) in* _allowed_dirs_ *or*
_allowed_files_. *$HOME*, *$XDG_CONFIG_HOME*, *~/.local*, and *~/.local/share* are
permanently forbidden (exact match only -- children can be mounted).
*$XDG_CONFIG_HOME/celily*, *~/.ssh*, *~/.gnupg*, and
*~/.local/share/keyrings* are permanently forbidden along with everything
underneath them regardless of the allowlists.

*source* = _path_ (required)
	Host directory or file to mount. A leading *~* expands to the host
	home directory. The resolved path must be listed exactly in
	_allowed_dirs_ (for directories) or _allowed_files_ (for files).

*target* = _path_ (required)
	Mount point inside the container. A leading *~* expands to the container
	home directory (_/home/_*user* or _/root_ with *--root*).

*access* = _string_ (optional)
	Set to *"readwrite"* for read-write access. Default: *"readonly"*.

Example:
```
[[mounts]]
source = "~/.pi"
target = "~/.pi"
access = "readwrite"

[[mounts]]
source = "~/Projects/client"
target = "/mnt/client"
```

# [network] SECTION

Network isolation is mandatory. Every *celily* invocation creates a
per-instance LXD/Incus bridge with an egress ACL and a per-instance mitmproxy
process that enforces the allowlist.

- A dedicated bridge is created for the instance with an egress ACL
	that blocks all TCP traffic except connections to the proxy.
	DNS (UDP+TCP 53) is restricted to the bridge gateway by default
	(_dns_ defaults to true). Set _dns = false_ to allow DNS to
	any destination.
- A per-instance mitmdump process runs on the host, bound to the
	bridge gateway IP. It provides an HTTP forward proxy and a DNS
	server. The container uses it as an explicit HTTP forward proxy
	and as its primary DNS resolver.
- *HTTP_PROXY*, *HTTPS_PROXY*, and *NO_PROXY* environment
	variables are forcibly injected into the container at the highest
	priority -- they cannot be overridden by *--env* or config.
- A per-instance CA certificate is pushed into the container so
	TLS-terminated connections through the proxy are trusted.

If the agent ignores the proxy environment variables and attempts
a direct connection to the internet on port 80 or 443, the bridge
ACL drops the packet. The proxy is the only working path to
HTTP/HTTPS.

*dns* = _bool_ (optional)
	Enable DNS filtering. When true (default), the bridge ACL restricts DNS
	to the gateway only and the bridge's dnsmasq is configured to forward
	all queries to mitmdump's DNS listener (on a high port, no privileged
	port required). The allowlist is enforced on every query (hostname match
	only; path prefixes are ignored). Set to false to allow DNS to any
	destination with the proxy DNS listener not filtering.

*Note:* DNS filtering uses exact hostname matching.
Subdomains do not inherit rules from their parent domain.
For example, allowing *example.com* does not match
*api.example.com* -- each subdomain must have its own
allow rule.

## [[network.allow]]

Array of allowed hosts/paths (HTTP) or IP/port pairs (TCP).
Any request not matching an allow rule is blocked (HTTP 403 for
HTTP traffic, nftables reject for TCP traffic).
Rules are matched in config-file order; first match wins. For
overlapping _path_prefixes_ values on the same host, put
more-specific rules first.

*type* = _string_ (required)
	Rule type. Supported values:
	- *"http"* -- HTTP/HTTPS host rules (see below). Enforced by the per-instance mitmproxy.
	- *"tcp"* -- Raw TCP destination rules (see TCP RULES below). Enforced by the bridge egress ACL (nftables).

*HTTP rules* (type = "http")

*host* = _string_ (required)
	Hostname to allow (e.g. *api.example.com*). Matched against the
	HTTP Host header. Matching is case-insensitive -- hostnames from
	config are normalized to lowercase before comparison.

*path_prefixes* = _[strings]_ (optional)
	Path prefixes to restrict the rule to (e.g. _["/v2"]_). Each
	prefix is matched with _startswith_ -- _/v2_ matches
	_/v2/foo_ but not _/v20_. The rule matches if the request
	path starts with any of the listed prefixes.
	When absent, the entire host is allowed.

*methods* = _[strings]_ (optional)
	Allowed HTTP methods (e.g. _["GET", "HEAD"]_). When absent,
	defaults to _["GET", "HEAD"]_. When present, only requests whose method
	appears in the list match the rule -- other methods fall through
	to the next rule or the default deny.

Example with non-default methods:
```
[[network.allow]]
type = "http"
host = "api.example.com"
methods = ["GET", "HEAD", "POST", "PUT"]
```

*HTTP auth sub-table* (optional, on HTTP [[network.allow]])

When present, the specified HTTP header is injected by mitmdump on
matching requests. The secret never appears in the container's
environment.

*secret* = _string_ (required)
	Name of an *rbw*(1) item to resolve.

*header* = _string_ (required)
	HTTP header name to inject (e.g. *Authorization*, *x-api-key*).

*prefix* = _string_ (optional)
	Prepended to the resolved secret value. For
	*Authorization: Bearer <key>*, set _prefix = "Bearer "_.
	Omit for headers expecting the bare key.

*HTTP headers table* (optional, on HTTP [[network.allow]])

When present, the specified static HTTP headers are injected by mitmdump
on matching requests. Values are plain text (no secret resolution).

_key_ = _value_
	Each key is an HTTP header name (e.g. *X-Custom-ID*) and the value
	is its literal string. Use this for headers that do not need secret
	resolution. For secret-backed injection, use _auth_.

*HTTP quota sub-table* (optional, on HTTP [[network.allow]])

When present, requests matching this allow rule are counted against a
request quota with a fixed time window. Exceeded quotas return HTTP 429.
Quotas use fixed time buckets aligned to wall clock -- the counter
resets when the bucket rolls over.

*max_requests* = _integer_ (required)
	Maximum number of HTTP requests allowed in the time window. Must be > 0.

*window* = _string_ (required)
	Time window duration. Accepts compact forms like *1h*, *30m*, *86400s*, *7d*
	as well as long forms (*1hour 30min*), combined units (*2h 15m*), and
	fractional values (*1.5h*). Suffixes: *ns*, *us/µs*, *ms*, *s/sec*, *m/min*,
	*h/hr*, *d/day*, *w/week*, *M/month*, *y/year* (and their plural/long
	variants). See https://github.com/chronotope/humantime for the entire rules.

Example:
```
[[network.allow]]
type = "http"
host = "api.openai.com"
quota = { max_requests = 1000, window = "1h" }
```

*TCP rules* (type = "tcp")

TCP rules allow raw TCP connections to specific IP addresses and ports.
Traffic bypasses the HTTP proxy entirely -- enforcement is at the
bridge egress ACL (nftables).

*host* = _string_ (required)
	Destination IP address (IPv4 or IPv6). Hostnames are not supported
	\-- the value is passed verbatim to nftables and must be a valid IP.

*ports* = _[integer]_ (required)
	Destination TCP ports. Must be non-empty. Each value is a single
	port number; port ranges are not supported. All ports for a given
	host are combined into a single ACL rule.

*Known limitations:*
	- IP addresses only (no DNS/hostname resolution).
	- No port-range syntax (list individual ports).

Example:
```
[network]

[[network.allow]]
type = "http"
host = "api.github.com"
methods = ["GET", "HEAD"]

[[network.allow]]
type = "http"
host = "api.openai.com"
path_prefixes = ["/v1"]
headers = { "User-Agent" = "celily/1.0" }
auth = { secret = "openai-api-key", header = "Authorization", prefix = "Bearer " }
quota = { max_requests = 1000, window = "1h" }

[[network.allow]]
type = "http"
host = "docs.github.com"

[[network.allow]]
type = "tcp"
host = "10.0.0.5"
ports = [22, 8443]
```

# [env] SECTION

Static environment variables set for every command run inside the container.
CLI *--env* overrides take precedence.

Example:
```
[env]
CARGO_TERM_COLOR = "always"
```

# ENVIRONMENT

*XDG_CONFIG_HOME*
	Configuration directory. Defaults to *$HOME/.config* if unset.
	celily reads all configuration from *$XDG_CONFIG_HOME/celily/*.

*XDG_STATE_HOME*
	State and log directory. Defaults to *$HOME/.local/state* if unset.
	mitmproxy logs are written to *$XDG_STATE_HOME/celily/logs/*.

*HOME*
	Home directory. Used as the base for forbidden path checks in
	mount validation and as the default for XDG directories above.

# SEE ALSO

*celily*(1)
