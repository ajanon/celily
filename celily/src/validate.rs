use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Error returned when a path matches a [`Forbidden`] entry.
#[derive(Debug, thiserror::Error)]
pub enum ForbiddenError {
    #[error("{label} is forbidden")]
    Exact { label: &'static str },
    #[error("path is under {label} (forbidden)")]
    Subtree { label: &'static str },
}
use celily_lib::AsyncCommandExt;

use crate::util::is_under_or_eq;

/// A path that may not be bind-mounted into the container.
///
/// `Exact` forbids only the path itself - children remain mountable.
/// `Subtree` forbids the path and everything underneath it.
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
            _ => Ok(()),
        }
    }
}

/// Validate that a mount source is authorized before binding it into the
/// container.
///
/// Security rules (enforced in order):
///
/// 1. At least one of `allowed_dirs` or `allowed_files` must be non-empty (set
///    in the config at `allowed_dirs` / `allowed_files`).
/// 2. The canonical path must pass every entry in `forbidden` - any match
///    (exact or subtree, depending on the variant) causes rejection.
/// 3. The canonical path must be under `home` - mounting outside `$HOME` is
///    rejected.
/// 4. If the path is a directory, it must be an exact match for an entry in
///    `allowed_dirs` (which must be pre-canonicalized by the caller). If it is
///    a file, it must be an exact match for an entry in `allowed_files`
///    (pre-canonicalized). Subtree matching is intentionally rejected - only
///    explicitly listed paths can be mounted. Symlinks, sockets, and other
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

    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))?;

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
/// 2. The extracted path must canonicalize successfully.
/// 3. The canonical path must pass every entry in the proxy-forbidden list
/// 4. The canonical path must be under `$HOME` or the user's `$XDG_RUNTIME_DIR`
///    (defaulting to `/run/user/<uid>`).
pub fn validate_proxy_connect(connect: &str, home: &Path, uid: u32) -> Result<()> {
    let path_str = connect
        .strip_prefix("unix:")
        .context("proxy connect must start with `unix:` (only Unix sockets are supported)")?;

    let path = Path::new(path_str);
    let canonical = path
        .canonicalize()
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

    Ok(())
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
/// 4. The resolved branch name is valid (`git check-ref-format --branch`).
///
/// All checks run on the host before touching LXD.
pub async fn validate_worktree_preconditions(
    project_dir: &Path,
    resolved_branch: &str,
) -> Result<()> {
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

    // Validate the resolved branch name against git's rules.
    tokio::process::Command::new("git")
        .args(["check-ref-format", "--branch", resolved_branch])
        .run()
        .await
        .with_context(|| format!("invalid worktree branch name '{resolved_branch}'"))?;

    Ok(())
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

        // ~/.local/share/applications is NOT blocked - ~/.local/share is Exact-blocked
        // but children (other than keyrings/) are fine
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
}
