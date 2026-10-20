use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Error returned when a path matches a [`Forbidden`] entry.
#[derive(Debug, thiserror::Error)]
pub enum ForbiddenError {
    #[error("{label} is forbidden")]
    Exact { label: &'static str },
    #[error("path is under {label} (forbidden)")]
    Subtree { label: &'static str },
    #[error("path contains {label} (forbidden)")]
    Contains { label: &'static str },
}
use celily_lib::{AccessMode, AsyncCommandExt, CommandError, Mount};

use crate::util::{git_config, is_under_or_eq};

/// A path that may not be bind-mounted into the container.
///
/// `Exact` forbids only the path itself - children remain mountable.
/// `Subtree` forbids the path, everything underneath it, and every path
/// containing it: a symlinked layout (`~/.ssh -> ~/dotfiles/ssh`) can put
/// it under a directory that is not forbidden itself.
///
/// Constructors `exact()` / `under()` handle canonicalization and can return
/// `None` if the path does not exist (graceful skip).
pub enum Forbidden {
    Exact {
        canonical: PathBuf,
        label: &'static str,
    },
    Subtree {
        canonical: PathBuf,
        label: &'static str,
    },
}

impl Forbidden {
    /// Forbid only the exact canonical path.
    /// The caller must pass an already-canonicalized path.
    pub fn exact(canonical: PathBuf, label: &'static str) -> Self {
        Self::Exact { canonical, label }
    }

    /// Forbid the directory joined from `base`/`segment` and all paths
    /// underneath. Returns `None` if the resulting path cannot be
    /// canonicalized (e.g. the directory does not exist).
    pub fn under(base: &Path, segment: &str, label: &'static str) -> Option<Self> {
        let canonical = base.join(segment).canonicalize().ok()?;
        Some(Self::Subtree { canonical, label })
    }

    /// Check whether a canonical path violates this forbidden entry.
    ///
    /// Returns `Ok(())` if allowed, or a [`ForbiddenError`] describing
    /// the violation.
    pub fn check(&self, canonical: &Path) -> Result<(), ForbiddenError> {
        match self {
            Forbidden::Exact {
                canonical: blocked,
                label,
            } if canonical == blocked => Err(ForbiddenError::Exact { label: *label }),
            Forbidden::Subtree {
                canonical: blocked,
                label,
            } if is_under_or_eq(canonical, blocked) => {
                Err(ForbiddenError::Subtree { label: *label })
            },
            Forbidden::Subtree {
                canonical: blocked,
                label,
            } if is_under_or_eq(blocked, canonical) => {
                Err(ForbiddenError::Contains { label: *label })
            },
            _ => Ok(()),
        }
    }
}

/// Canonicalize `path`, refusing it if it goes through a symlink inside
/// `home` (canonical). A sandbox with write access to a directory there could
/// swap one of its entries for a symlink, and the next run would follow it
/// out of the mount, or into an allowlist entry. Symlinks outside `home`
/// (`/home -> /var/home`) are not the sandbox's to plant, so they are fine.
///
/// Ceiling: checked at launch. A sandbox still running from another session
/// could swap a component between this check and LXD mounting the path.
pub fn canonicalize_in_home(path: &Path, home: &Path) -> Result<PathBuf> {
    let absolute =
        std::path::absolute(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    for ancestor in absolute.ancestors() {
        let in_home = || {
            ancestor
                .parent()
                .and_then(|parent| parent.canonicalize().ok())
                .is_some_and(|parent| is_under_or_eq(&parent, home))
        };
        if ancestor.is_symlink() && in_home() {
            bail!(
                "{} goes through the symlink {} (symlinks under $HOME are not followed)",
                path.display(),
                ancestor.display(),
            );
        }
    }
    absolute
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))
}

/// Validate that a mount source is authorized before binding it into the
/// container.
///
/// Security rules (enforced in order):
///
/// 1. At least one of `allowed_dirs` or `allowed_files` must be non-empty (set
///    in the config at `allowed_dirs` / `allowed_files`).
/// 2. The path must not go through a symlink inside `home`
///    ([`canonicalize_in_home`]).
/// 3. The canonical path must pass every entry in `forbidden` - any match
///    (exact or subtree, depending on the variant) causes rejection.
/// 4. The canonical path must be under `home` - mounting outside `$HOME` is
///    rejected.
/// 5. If the path is a directory, it must be an exact match for an entry in
///    `allowed_dirs` (which the caller must canonicalize with
///    [`canonicalize_in_home`]). If it is a file, it must be an exact match for
///    an entry in `allowed_files` (same). Subtree matching is intentionally
///    rejected - only explicitly listed paths can be mounted. Sockets and other
///    special files are rejected.
pub fn validate_mount_source(
    path: &Path,
    home: &Path,
    forbidden: &[Forbidden],
    allowed_dirs: &[PathBuf],
    allowed_files: &[PathBuf],
) -> Result<PathBuf> {
    if allowed_dirs.is_empty() && allowed_files.is_empty() {
        bail!(
            "allowed_dirs and allowed_files are empty: configure at least one in \
             ~/.config/celily/config.toml"
        );
    }

    let canonical = canonicalize_in_home(path, home)?;

    for entry in forbidden {
        entry
            .check(&canonical)
            .with_context(|| format!("cannot mount {}", path.display()))?;
    }

    if !is_under_or_eq(&canonical, home) {
        bail!("{} is not under {}", path.display(), home.display());
    }

    let is_dir = canonical.is_dir();
    let is_file = canonical.is_file();

    if is_dir {
        if !allowed_dirs.iter().any(|d| d == &canonical) {
            bail!(
                "directory {} is not in the allowed_dirs list (exact match required, subtrees are \
                 not allowed)",
                path.display(),
            );
        }
    } else if is_file {
        if !allowed_files.iter().any(|f| f == &canonical) {
            bail!(
                "file {} is not in the allowed_files list (exact match required)",
                path.display(),
            );
        }
    } else {
        bail!(
            "{} is neither a regular file nor a directory",
            path.display(),
        );
    }

    Ok(canonical)
}

/// Validate a proxy device's connect path against the proxy-specific
/// forbidden list and boundary rules.
///
/// The `connect` string must be in LXD/Incus format (e.g.
/// `unix:/run/user/1000/dbus-notifications.sock`). Only `unix:` sockets
/// are supported (no abstract sockets either).
///
/// Security rules (enforced in order):
///
/// 1. The connect string must start with `unix:`.
/// 2. The extracted path must canonicalize with [`canonicalize_in_home`].
/// 3. The canonical path must pass every entry in the proxy-forbidden list
/// 4. The canonical path must be under `$HOME` or the user's `$XDG_RUNTIME_DIR`
///    (defaulting to `/run/user/<uid>`).
/// 5. The canonical path must not be under a read-write mount of `mounts`.
///
/// LXD resolves the path again on every connection: hand it the canonical
/// path this returns, never `connect`. 5 keeps the sandbox from swapping the
/// socket for a symlink to another one during the session.
pub fn validate_proxy_connect(
    connect: &str,
    home: &Path,
    uid: u32,
    mounts: &[Mount],
) -> Result<PathBuf> {
    let path_str = connect
        .strip_prefix("unix:")
        .context("proxy connect must start with `unix:` (only Unix sockets are supported)")?;

    let canonical = canonicalize_in_home(Path::new(path_str), home)
        .with_context(|| format!("cannot resolve proxy socket {path_str}"))?;

    let runtime_dir_raw = PathBuf::from(
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{uid}")),
    );
    let runtime_dir = runtime_dir_raw.canonicalize().unwrap_or(runtime_dir_raw);

    let forbidden = build_proxy_forbidden_list(home, &runtime_dir);
    for entry in &forbidden {
        entry
            .check(&canonical)
            .with_context(|| format!("cannot proxy socket {path_str}"))?;
    }

    if !is_under_or_eq(&canonical, home) && !is_under_or_eq(&canonical, &runtime_dir) {
        bail!(
            "proxy socket {path_str} is not under {} or {}",
            home.display(),
            runtime_dir.display(),
        );
    }

    let writable = mounts
        .iter()
        .find(|m| m.access == AccessMode::ReadWrite && is_under_or_eq(&canonical, &m.source));
    if let Some(mount) = writable {
        bail!(
            "proxy socket {path_str} is under the read-write mount {}: the sandbox could replace \
             it",
            mount.source.display(),
        );
    }

    Ok(canonical)
}

/// Build the forbidden-path list for proxy devices.
///
/// Directories and sockets that are always too sensitive to expose
fn build_proxy_forbidden_list(home: &Path, runtime_dir: &Path) -> Vec<Forbidden> {
    let mut list = Vec::new();

    // SSH agent sockets (in home directory).
    list.extend(Forbidden::under(home, ".ssh/agent", "~/.ssh/agent"));

    // Secrets/credential brokers under $XDG_RUNTIME_DIR.
    list.extend(Forbidden::under(
        runtime_dir,
        "gnupg",
        "$XDG_RUNTIME_DIR/gnupg",
    ));
    list.extend(Forbidden::under(
        runtime_dir,
        "keyring",
        "$XDG_RUNTIME_DIR/keyring",
    ));
    list.extend(Forbidden::under(runtime_dir, "rbw", "$XDG_RUNTIME_DIR/rbw"));
    list.extend(Forbidden::under(runtime_dir, "gcr", "$XDG_RUNTIME_DIR/gcr"));
    list.extend(Forbidden::under(
        runtime_dir,
        "p11-kit",
        "$XDG_RUNTIME_DIR/p11-kit",
    ));

    // Session-control and container control
    list.extend(Forbidden::under(
        runtime_dir,
        "systemd",
        "$XDG_RUNTIME_DIR/systemd",
    ));
    list.extend(Forbidden::under(
        runtime_dir,
        "podman",
        "$XDG_RUNTIME_DIR/podman",
    ));

    // D-Bus session bus
    let bus_path = runtime_dir.join("bus");
    if let Ok(canon) = bus_path.canonicalize() {
        list.push(Forbidden::exact(canon, "$XDG_RUNTIME_DIR/bus"));
    }

    list
}

/// Validate that the project directory can support worktree mode.
///
/// Checks (in order):
/// 1. `.git` exists and is a real directory (not symlink, not a regular file).
/// 2. No `.gitmodules` (submodules are not supported).
/// 3. The repository has at least one commit (`git rev-parse HEAD`).
/// 4. The branch name is valid (`git check-ref-format --branch`).
/// 5. git's `ext::` transport policy is `user`.
///
/// All checks run on the host before touching LXD.
pub async fn validate_worktree_preconditions(project_dir: &Path, branch: &str) -> Result<()> {
    let dotgit = project_dir.join(".git");

    if !dotgit.exists() {
        bail!(
            "worktree mode requires a git repository (no .git directory found in {})",
            project_dir.display(),
        );
    }

    if dotgit.is_symlink() {
        bail!(
            "worktree mode requires .git to be a directory (found symlink at {})",
            dotgit.display(),
        );
    }

    if !dotgit.is_dir() {
        bail!(
            "worktree mode requires .git to be a directory (found file at {})",
            dotgit.display(),
        );
    }

    let gitmodules = project_dir.join(".gitmodules");
    if gitmodules.exists() {
        bail!(
            "worktree mode does not support submodules (.gitmodules detected in {})",
            project_dir.display(),
        );
    }

    // Verify the repository has at least one commit.
    tokio::process::Command::new("git")
        .args(["-C"])
        .arg(project_dir)
        .args(["rev-parse", "HEAD"])
        .run_stdout()
        .await
        .context("worktree mode requires at least one commit in the repository")?;

    // Validate the branch name against git's rules.
    tokio::process::Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .run()
        .await
        .with_context(|| format!("invalid worktree branch name '{branch}'"))?;

    check_ext_transport_allowed(project_dir).await?;
    warn_if_fetch_fsck_disabled(project_dir).await;

    Ok(())
}

/// Worktree mode fetches through git's `ext::` transport: require `user`.
/// `always` would also run `ext::` URLs from fetched commits, such as a
/// submodule URL. Checked at launch only.
async fn check_ext_transport_allowed(project_dir: &Path) -> Result<()> {
    let ext_allow = config_entries(project_dir, "protocol.ext.allow").await?;
    let allow = config_entries(project_dir, "protocol.allow").await?;
    // Lossy, not `var`: a non-UTF-8 value must not read as unset.
    let allow_protocol =
        std::env::var_os("GIT_ALLOW_PROTOCOL").map(|v| v.to_string_lossy().into_owned());
    let (policy, source) = ext_policy(&ext_allow, &allow, allow_protocol.as_deref());
    // git compares these values case-insensitively.
    match policy.to_ascii_lowercase().as_str() {
        "user" => Ok(()),
        "always" => bail!(
            "git allows its ext:: transport 'always' ({source}). The sandbox could commit a \
             .gitmodules with an ext:: URL, and `git submodule update --init` would run its \
             command on this host. Use 'user' instead; see WORKTREE MODE in celily(1)."
        ),
        _ => bail!(
            "worktree mode fetches from the sandbox through git's ext:: transport, which git does \
             not allow here ({source}). Allow it for this repository with:\n\n    git -C {} \
             config protocol.ext.allow user\n\nSee WORKTREE MODE in celily(1).",
            project_dir.display(),
        ),
    }
}

/// git's `ext` policy and where it comes from. `GIT_ALLOW_PROTOCOL`
/// overrides the config. A submodule clone does not read this repository's
/// config, so a local `user` must not hide an `always` the submodule sees.
fn ext_policy(
    ext_allow: &[ConfigEntry],
    allow: &[ConfigEntry],
    allow_protocol: Option<&str>,
) -> (String, String) {
    if let Some(list) = allow_protocol {
        let policy = if list.split(':').any(|p| p == "ext") {
            "always"
        } else {
            "never"
        };
        return (policy.to_owned(), format!("GIT_ALLOW_PROTOCOL={list}"));
    }
    // git's lookup: the last `protocol.ext.allow`, else the last
    // `protocol.allow`.
    let lookup = |keep: fn(&str) -> bool| {
        ext_allow
            .iter()
            .rfind(|e| keep(&e.scope))
            .or_else(|| allow.iter().rfind(|e| keep(&e.scope)))
    };
    let entry = lookup(|scope| !matches!(scope, "local" | "worktree"))
        .filter(|e| e.value.eq_ignore_ascii_case("always"))
        .or_else(|| lookup(|_| true));
    match entry {
        Some(e) => (
            e.value.clone(),
            format!("{} = {}, {} scope, {}", e.key, e.value, e.scope, e.origin),
        ),
        None => ("never".to_owned(), "never, git's default".to_owned()),
    }
}

/// One line of `git config --show-scope --show-origin --get-all`.
#[derive(Debug, PartialEq)]
struct ConfigEntry {
    key: &'static str,
    scope: String,
    origin: String,
    value: String,
}

/// All values of `key`, in git's read order. Fails closed: any exit code
/// but 1 (unset) is an error.
async fn config_entries(dir: &Path, key: &'static str) -> Result<Vec<ConfigEntry>> {
    let listing = match tokio::process::Command::new("git")
        // Only `git config` reads GIT_CONFIG, as its sole file: it would
        // hide the config the rest of git uses.
        .env_remove("GIT_CONFIG")
        .arg("-C")
        .arg(dir)
        .args(["config", "--show-scope", "--show-origin", "--get-all", key])
        .run_stdout()
        .await
    {
        Ok(listing) => listing,
        Err(CommandError::NonZero { code: Some(1), .. }) => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {key} from the git config")),
    };
    parse_config_entries(key, &listing)
}

fn parse_config_entries(key: &'static str, listing: &str) -> Result<Vec<ConfigEntry>> {
    listing
        .lines()
        .map(|line| {
            let mut fields = line.splitn(3, '\t').map(str::to_owned);
            match (fields.next(), fields.next(), fields.next()) {
                (Some(scope), Some(origin), Some(value)) => Ok(ConfigEntry {
                    key,
                    scope,
                    origin,
                    value,
                }),
                _ => bail!("unexpected `git config` output for {key}: {line:?}"),
            }
        })
        .collect()
}

/// Fetched objects are always hash-checked; `fetch.fsckObjects` adds checks
/// for malformed objects and dangerous paths. Recommended, not required.
///
/// Mirrors git's lookup: `fetch.fsckObjects`, then `transfer.fsckObjects`.
async fn warn_if_fetch_fsck_disabled(project_dir: &Path) {
    let fsck =
        if let Some(fsck) = git_config(project_dir, &["--type=bool", "fetch.fsckObjects"]).await {
            Some(fsck)
        } else {
            git_config(project_dir, &["--type=bool", "transfer.fsckObjects"]).await
        };
    if fsck.as_deref() != Some("true") {
        tracing::warn!(
            "fetch.fsckObjects is off: objects fetched from the sandbox are hash-checked but not \
             fsck'd. Recommended: git -C {} config fetch.fsckObjects true",
            project_dir.display(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_forbidden_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let config = home.join(".config");
        let celily_dir = config.join("celily");
        let nvim_dir = config.join("nvim");
        let projects_dir = home.join("Projects");
        let ssh_dir = home.join(".ssh");
        let gnupg_dir = home.join(".gnupg");
        let local_dir = home.join(".local");
        let local_share_dir = local_dir.join("share");
        let keyrings_dir = local_share_dir.join("keyrings");
        let local_bin_dir = local_dir.join("bin");
        let local_share_apps_dir = local_share_dir.join("applications");

        std::fs::create_dir_all(home.join("Projects")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&celily_dir).unwrap();
        std::fs::write(celily_dir.join("config.toml"), "").unwrap();
        std::fs::create_dir_all(&nvim_dir).unwrap();
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::create_dir_all(&ssh_dir).unwrap();
        std::fs::write(ssh_dir.join("id_rsa"), "fake-key").unwrap();
        std::fs::create_dir_all(&gnupg_dir).unwrap();
        std::fs::write(gnupg_dir.join("secring.gpg"), "fake-gpg").unwrap();
        std::fs::create_dir_all(&keyrings_dir).unwrap();
        std::fs::write(keyrings_dir.join("login.keyring"), "fake-keyring").unwrap();
        std::fs::create_dir_all(&local_bin_dir).unwrap();
        std::fs::create_dir_all(&local_share_apps_dir).unwrap();

        let canon_home = canonic_path(&home);
        let canon_config = canonic_path(&config);

        let forbidden = vec![
            Forbidden::exact(canon_home.clone(), "$HOME"),
            Forbidden::exact(canon_config.clone(), "~/.config"),
            Forbidden::exact(canonic_path(&local_dir), "~/.local"),
            Forbidden::exact(canonic_path(&local_share_dir), "~/.local/share"),
            Forbidden::under(&canon_config, "celily", "~/.config/celily").unwrap(),
            Forbidden::under(&canon_home, ".ssh", "~/.ssh").unwrap(),
            Forbidden::under(&canon_home, ".gnupg", "~/.gnupg").unwrap(),
            Forbidden::under(
                &canon_home,
                ".local/share/keyrings",
                "~/.local/share/keyrings",
            )
            .unwrap(),
        ];

        // The allowlist includes blocked paths to verify that the forbidden
        // list takes priority - these are all hard-blocked regardless.
        let canon_allowed = vec![
            canonic_path(&projects_dir),
            canonic_path(&nvim_dir),
            canonic_path(&local_bin_dir),
            canonic_path(&local_share_apps_dir),
            canonic_path(&config),
            canonic_path(&celily_dir),
            canonic_path(&ssh_dir),
            canonic_path(&gnupg_dir),
            canonic_path(&local_dir),
            canonic_path(&local_share_dir),
            canonic_path(&keyrings_dir),
        ];
        let empty_files: Vec<PathBuf> = vec![];

        // Reject $HOME directly
        let err =
            validate_mount_source(&home, &canon_home, &forbidden, &canon_allowed, &empty_files)
                .unwrap_err();
        assert!(format!("{err:#}").contains("$HOME is forbidden"));

        // Reject ~/.config directly
        let err = validate_mount_source(
            &config,
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.config is forbidden"));

        // Reject ~/.config/celily
        let err = validate_mount_source(
            &celily_dir,
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.config/celily"));

        // Reject anything under ~/.config/celily
        let err = validate_mount_source(
            &celily_dir.join("config.toml"),
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.config/celily"));

        // Reject ~/.ssh directly
        let err = validate_mount_source(
            &ssh_dir,
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.ssh"));

        // Reject file under ~/.ssh
        let err = validate_mount_source(
            &ssh_dir.join("id_rsa"),
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.ssh"));

        // Reject ~/.gnupg directly
        let err = validate_mount_source(
            &gnupg_dir,
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.gnupg"));

        // Reject file under ~/.gnupg
        let err = validate_mount_source(
            &gnupg_dir.join("secring.gpg"),
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.gnupg"));

        // Reject ~/.local directly
        let err = validate_mount_source(
            &local_dir,
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.local is forbidden"));

        // Reject ~/.local/share directly
        let err = validate_mount_source(
            &local_share_dir,
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.local/share is forbidden"));

        // Reject ~/.local/share/keyrings
        let err = validate_mount_source(
            &keyrings_dir,
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.local/share/keyrings"));

        // Reject file under ~/.local/share/keyrings
        let err = validate_mount_source(
            &keyrings_dir.join("login.keyring"),
            &canon_home,
            &forbidden,
            &canon_allowed,
            &empty_files,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("~/.local/share/keyrings"));

        // ~/.local/bin is NOT blocked - only ~/.local itself is Exact-blocked
        assert!(
            validate_mount_source(
                &local_bin_dir,
                &canon_home,
                &forbidden,
                &canon_allowed,
                &empty_files
            )
            .is_ok()
        );

        // ~/.local/share/applications is NOT blocked - ~/.local/share is
        // Exact-blocked but children (other than keyrings/) are fine
        assert!(
            validate_mount_source(
                &local_share_apps_dir,
                &canon_home,
                &forbidden,
                &canon_allowed,
                &empty_files
            )
            .is_ok()
        );

        // ~/.config/nvim is allowed (if in allowlist)
        assert!(
            validate_mount_source(
                &nvim_dir,
                &canon_home,
                &forbidden,
                &canon_allowed,
                &empty_files
            )
            .is_ok()
        );

        // Regular directory under home is allowed
        assert!(
            validate_mount_source(
                &projects_dir,
                &canon_home,
                &forbidden,
                &canon_allowed,
                &empty_files
            )
            .is_ok()
        );
    }

    fn canonic_path(path: &Path) -> PathBuf {
        path.canonicalize().unwrap()
    }

    #[test]
    fn validate_rejects_mounts_containing_a_forbidden_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let home = canonic_path(tmp.path());
        let dotfiles = home.join("dotfiles");
        std::fs::create_dir_all(dotfiles.join("ssh")).unwrap();
        std::os::unix::fs::symlink(dotfiles.join("ssh"), home.join(".ssh")).unwrap();
        let forbidden = vec![Forbidden::under(&home, ".ssh", "~/.ssh").unwrap()];

        let err = validate_mount_source(
            &dotfiles,
            &home,
            &forbidden,
            std::slice::from_ref(&dotfiles),
            &[],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("path contains ~/.ssh"));
    }

    #[test]
    fn proxy_socket_is_canonical_and_not_under_a_writable_mount() {
        let dir = tempfile::tempdir().unwrap();
        let home = canonic_path(dir.path());
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(run.join("sock")).unwrap();
        let connect = format!("unix:{}/./run/sock", home.display());
        let mount = |access| Mount {
            source: run.clone(),
            target: PathBuf::from("/run/host"),
            access,
        };

        let socket = validate_proxy_connect(&connect, &home, 0, &[mount(AccessMode::ReadOnly)]);
        assert_eq!(socket.unwrap(), run.join("sock"));
        let err = validate_proxy_connect(&connect, &home, 0, &[mount(AccessMode::ReadWrite)])
            .unwrap_err();
        assert!(format!("{err:#}").contains("under the read-write mount"));
    }

    #[test]
    fn canonicalize_in_home_refuses_symlinks_under_home_only() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = canonic_path(dir.path());
        let home = tmp.join("home");
        std::fs::create_dir_all(home.join("work/real")).unwrap();
        std::fs::create_dir_all(home.join(".config/gh")).unwrap();
        // A sandbox with work/ read-write swaps an allowlisted entry.
        std::os::unix::fs::symlink(home.join(".config/gh"), home.join("work/swapped")).unwrap();
        // A symlinked $HOME is not the sandbox's doing.
        std::os::unix::fs::symlink(&home, tmp.join("home-link")).unwrap();

        let err = canonicalize_in_home(&home.join("work/swapped"), &home).unwrap_err();
        assert!(format!("{err:#}").contains("goes through the symlink"));
        let err = canonicalize_in_home(&home.join("work/swapped/hosts.yml"), &home).unwrap_err();
        assert!(format!("{err:#}").contains("goes through the symlink"));
        assert_eq!(
            canonicalize_in_home(&tmp.join("home-link/work/real"), &home).unwrap(),
            home.join("work/real"),
        );
    }

    /// Cases checked against a real `git submodule update --init`.
    #[test]
    fn ext_policy_follows_git_lookup() {
        let policy = |ext_allow: &str, allow: &str, allow_protocol: Option<&str>| {
            let ext_allow = parse_config_entries("protocol.ext.allow", ext_allow).unwrap();
            let allow = parse_config_entries("protocol.allow", allow).unwrap();
            ext_policy(&ext_allow, &allow, allow_protocol).0
        };
        assert_eq!(policy("", "", None), "never");
        assert_eq!(policy("global\tg\tuser", "", None), "user");
        // A local `user` does not shield a submodule (the command ran).
        assert_eq!(
            policy("global\tg\talways\nlocal\tl\tuser", "", None),
            "always"
        );
        assert_eq!(
            policy("local\tl\tuser", "global\tg\talways", None),
            "always"
        );
        // protocol.ext.allow wins over protocol.allow.
        assert_eq!(policy("global\tg\tuser", "global\tg\talways", None), "user");
        // Command line config reaches submodules, and is read last.
        assert_eq!(
            policy("global\tg\tuser\ncommand\tc\tAlways", "", None),
            "Always"
        );
        // Harmless for submodules, refused anyway.
        assert_eq!(policy("local\tl\talways", "", None), "always");
        assert_eq!(policy("local\tl\tuser", "", Some("file:ext")), "always");
        assert_eq!(policy("local\tl\tuser", "", Some("file:git")), "never");
    }

    #[test]
    fn parse_config_entries_fails_closed() {
        assert!(parse_config_entries("k", "always").is_err());
        assert!(parse_config_entries("k", "global\talways").is_err());
    }

    /// Made by hand: no git command writes, even with `GIT_DIR` set.
    fn hand_made_repo(config: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dotgit = tmp.path().join(".git");
        std::fs::create_dir_all(dotgit.join("objects")).unwrap();
        std::fs::create_dir_all(dotgit.join("refs")).unwrap();
        std::fs::write(dotgit.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(dotgit.join("config"), config).unwrap();
        tmp
    }

    /// Real `git config` output, on keys set nowhere else.
    #[tokio::test]
    async fn config_entries_reads_git_config() {
        let repo = hand_made_repo("[celily-test]\n\tkey = one\n\tkey = two words\n");
        let entries = config_entries(repo.path(), "celily-test.key")
            .await
            .unwrap();
        let entry = |value: &str| ConfigEntry {
            key: "celily-test.key",
            scope: "local".to_owned(),
            origin: "file:.git/config".to_owned(),
            value: value.to_owned(),
        };
        assert_eq!(entries, [entry("one"), entry("two words")]);
        let unset = config_entries(repo.path(), "celily-test.unset").await;
        assert_eq!(unset.unwrap(), []);
    }

    #[tokio::test]
    async fn requires_ext_user() {
        let check = |policy: &str| {
            let repo = hand_made_repo(&format!("[protocol \"ext\"]\n\tallow = {policy}\n"));
            async move { check_ext_transport_allowed(repo.path()).await }
        };
        assert!(check("never").await.is_err());
        let err = check("always").await.unwrap_err();
        assert!(format!("{err:#}").contains("'always'"), "{err:#}");
    }
}
