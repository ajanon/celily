use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use celily_lib::{CommandExt, InstanceKind, Mount, NetworkRule};
use crate::config::{Config, WorktreeConfig};

use crate::cli::Args;
use crate::util::{
    expand_container_tilde,
    expand_host_tilde,
    instance_name,
    is_under_or_eq,
    is_valid_username,
    shell_escape,
};
use crate::validate::{Forbidden, validate_mount_source};

/// All configuration and runtime state resolved from CLI args, config files,
/// and host environment before launching the LXD instance.
pub struct RunContext {
    pub image: String,
    pub kind: InstanceKind,
    pub cpu: u32,
    pub memory: String,
    pub disk: String,
    pub processes: i32,
    pub disk_priority: Option<u32>,
    pub memory_enforce: Option<String>,
    pub network_ingress: Option<String>,
    pub network_egress: Option<String>,
    pub exec_uid: u32,
    pub exec_gid: u32,
    pub container_uid: u32,
    pub container_gid: u32,
    pub project_dir: PathBuf,
    pub container_home: PathBuf,
    pub raw_idmap: Option<String>,
    pub mounts: Vec<Mount>,
    pub env_map: HashMap<String, String>,
    pub name: String,
    pub ephemeral: bool,
    pub description: String,
    /// Random identifier shared between the instance name and bridge name.
    pub instance_uuid: uuid::Uuid,
    /// Network allow rules from config.
    pub network_allow: Vec<NetworkRule>,
    /// Whether DNS filtering is enabled (forces DNS through proxy with
    /// allowlist enforcement).
    pub network_dns: bool,
    /// Whether worktree mode is enabled for this run.
    pub worktree_enabled: bool,
    /// Whether the project directory should be mounted read-only.
    pub effective_readonly: bool,
    /// Enable security.nesting (required for snapd, Docker, etc.).
    pub security_nesting: bool,
    /// Disable UEFI secure boot for VMs. Ignored on containers.
    /// Default: true (LXD default).
    pub secure_boot: bool,
    /// Whether to bind-mount the notification proxy socket.
    pub notifications: bool,
}

/// Resolve CLI args, config, and host state into a [`RunContext`].
///
/// Validates all mount sources against `allowed_dirs`/`allowed_files`,
/// the forbidden-path list, and the `$HOME` boundary before returning.
pub fn resolve_context(
    args: &Args,
    cfg: &Config,
    cwd: &Path,
    home: &Path,
    home_canon: &Path,
    config_dir: &Path,
    allowed_dirs: &[PathBuf],
    allowed_files: &[PathBuf],
) -> Result<RunContext> {
    let image = args
        .image
        .clone()
        .or(cfg.image.clone())
        .unwrap_or_else(|| "celily".to_string());
    let kind = if args.vm {
        InstanceKind::Vm
    } else {
        cfg.kind.unwrap_or_default()
    };
    let cpu = args.cpu.or(cfg.limits.cpu).unwrap_or(2);
    let memory = args
        .memory
        .clone()
        .or(cfg.limits.memory.clone())
        .unwrap_or_else(|| "4GiB".to_string());
    let disk = args
        .disk
        .clone()
        .or(cfg.limits.disk.clone())
        .unwrap_or_else(|| "4GiB".to_string());
    let processes = args.processes.or(cfg.limits.processes).unwrap_or(1024);
    let disk_priority = cfg.limits.disk_priority;
    let memory_enforce = cfg.limits.memory_enforce.clone();
    let network_ingress = cfg.limits.network_ingress.clone();
    let network_egress = cfg.limits.network_egress.clone();

    let container_uid = args
        .container_uid
        .or(cfg.container_uid)
        .unwrap_or(1000);
    let container_gid = args
        .container_gid
        .or(cfg.container_gid)
        .unwrap_or(1000);
    let user_name = args
        .user
        .clone()
        .or(cfg.user.clone())
        .unwrap_or_else(|| "dev".to_string());
    if !is_valid_username(&user_name) && !args.root {
        bail!(
            "invalid username '{user_name}': must match [a-z_][a-z0-9_-]* and be <=32 characters"
        );
    }
    let exec_uid: u32 = if args.root { 0 } else { container_uid };
    let exec_gid: u32 = if args.root { 0 } else { container_gid };

    let container_home = if args.root {
        PathBuf::from("/root")
    } else {
        PathBuf::from(format!("/home/{user_name}"))
    };
    let project_target_str = args
        .project_target
        .clone()
        .or_else(|| cfg.project_target.clone())
        .unwrap_or_else(|| "~/project".to_string());

    let project_target = expand_container_tilde(Path::new(&project_target_str), &container_home);

    let raw_idmap = match kind {
        InstanceKind::Vm => None,
        InstanceKind::Container => {
            let host_uid = nix::unistd::Uid::current().as_raw();
            let host_gid = nix::unistd::Gid::current().as_raw();
            Some(format!(
                "uid {host_uid} {container_uid}\ngid {host_gid} {container_gid}"
            ))
        },
    };

    // --- Worktree / read-only / mount-project resolution ---
    let worktree_enabled = args.worktree.is_some();

    // Project mount: CLI flags take priority, then config, with a default
    // of false (no mount). Worktree mode forces it on regardless.
    let mount_project = worktree_enabled
        || args.mount_project
        || (!args.no_mount_project && cfg.mount_project.unwrap_or(false));

    let effective_cwd = if mount_project {
        project_target.clone()
    } else {
        container_home.clone()
    };

    let mut mounts: Vec<Mount> = Vec::new();
    if mount_project {
        mounts.push(Mount {
            source: cwd.to_path_buf(),
            target: project_target.clone(),
            readwrite: false,
        });
    }

    mounts.extend(cfg.mounts.iter().map(|m| Mount {
        source: expand_host_tilde(&m.source, home),
        target: expand_container_tilde(&m.target, &container_home),
        readwrite: m.readwrite,
    }));
    mounts.extend(args.cli_mounts.iter().map(|m| Mount {
        source: expand_host_tilde(&m.source, home),
        target: expand_container_tilde(&m.target, &container_home),
        readwrite: m.readwrite,
    }));

    // Pre-canonicalize allowlists once so validate_mount_source doesn't
    // re-canonicalize them for every mount. Non-existent entries are
    // skipped - they'll be rejected at mount time anyway.
    let canon_allowed_dirs: Vec<PathBuf> = allowed_dirs
        .iter()
        .map(|d| expand_host_tilde(d, home))
        .filter_map(|d| match d.canonicalize() {
            Ok(canon) => Some(canon),
            Err(e) => {
                tracing::warn!("allowed_dir entry '{}' skipped: {e}", d.display(),);
                None
            },
        })
        .collect();
    let canon_allowed_files: Vec<PathBuf> = allowed_files
        .iter()
        .map(|f| expand_host_tilde(f, home))
        .filter_map(|f| match f.canonicalize() {
            Ok(canon) => Some(canon),
            Err(e) => {
                tracing::warn!("allowed_file entry '{}' skipped: {e}", f.display(),);
                None
            },
        })
        .collect();

    // Build the forbidden-path list once. Paths that do not exist (e.g.
    // ~/.ssh on a machine without SSH keys) are silently skipped.
    let config_canon = config_dir.canonicalize().with_context(|| {
        format!(
            "cannot canonicalize config directory {}",
            config_dir.display()
        )
    })?;
    let mut forbidden = vec![
        Forbidden::exact(home_canon.to_path_buf(), "$HOME"),
        Forbidden::exact(config_canon.clone(), "~/.config"),
    ];
    if let Ok(local_canon) = home_canon.join(".local").canonicalize() {
        if let Ok(local_share_canon) = local_canon.join("share").canonicalize() {
            forbidden.push(Forbidden::exact(local_share_canon, "~/.local/share"));
        }
        forbidden.push(Forbidden::exact(local_canon, "~/.local"));
    }
    forbidden.extend(Forbidden::under(
        &config_canon,
        "celily",
        "~/.config/celily",
    ));
    forbidden.extend(Forbidden::under(home_canon, ".ssh", "~/.ssh"));
    forbidden.extend(Forbidden::under(home_canon, ".gnupg", "~/.gnupg"));
    forbidden.extend(Forbidden::under(
        home_canon,
        ".local/share/keyrings",
        "~/.local/share/keyrings",
    ));

    for mount in &mut mounts {
        let canonical = validate_mount_source(
            &mount.source,
            home_canon,
            &forbidden,
            &canon_allowed_dirs,
            &canon_allowed_files,
        )?;
        mount.source = canonical;
    }

    let mut env_map: HashMap<String, String> = cfg.env.clone();
    for name in cfg.pass_env.iter().chain(args.cli_pass_env.iter()) {
        if let Ok(val) = env::var(name) {
            env_map.insert(name.clone(), val);
        } else {
            tracing::warn!(
                var = %name,
                "pass_env variable is not set on the host; environment will fall back to static env or be empty",
            );
        }
    }
    for kv in &args.cli_env {
        let (k, v) = kv
            .split_once('=')
            .with_context(|| format!("invalid --env value '{kv}', expected KEY=VALUE"))?;
        env_map.insert(k.to_string(), v.to_string());
    }

    let instance_uuid = uuid::Uuid::new_v4();
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| instance_name(cwd, home, &image, &instance_uuid));
    let ephemeral = !args.keep_instance;
    let description = format!("celily run ({image} at {})", cwd.display());

    let network_allow: Vec<NetworkRule> = cfg
        .network
        .allow
        .iter()
        .cloned()
        .map(|r| r.into_library())
        .collect::<Result<Vec<_>, _>>()?;
    let network_dns = cfg.network.dns.unwrap_or(true);
    let security_nesting = cfg.security_nesting.unwrap_or(false);
    let secure_boot = cfg.secure_boot.unwrap_or(true);
    let notifications = cfg.notifications.unwrap_or(true);

    let effective_readonly = worktree_enabled
        || args.project_readonly
        || (!args.no_project_readonly && cfg.project_readonly.unwrap_or(false));

    if worktree_enabled {
        // Project mount is read-only by default; force it explicitly
        // when worktree mode is active (belt and suspenders).
        mounts[0].readwrite = false;
        // Overlay .git read-write. Placed second in the mounts list
        // so LXD applies it after the read-only project mount.
        //
        // Validate the .git path to prevent symlink bypass of mount
        // validation (finding V1).
        let git_source = cwd.join(".git");
        let git_canon = git_source
            .canonicalize()
            .with_context(|| format!("cannot resolve .git path: {}", git_source.display()))?;
        for entry in &forbidden {
            entry
                .check(&git_canon)
                .with_context(|| "cannot mount .git overlay")?;
        }
        if !is_under_or_eq(&git_canon, home_canon) {
            bail!(
                "cannot mount .git overlay: {} is not under {}",
                git_source.display(),
                home_canon.display(),
            );
        }
        // Ensure .git is still inside the already-validated project
        // directory (symlink escape guard).
        let cwd_canon = &mounts[0].source;
        if !is_under_or_eq(&git_canon, cwd_canon) {
            bail!(
                "cannot mount .git overlay: {} resolves outside the project directory",
                git_source.display(),
            );
        }
        mounts.insert(
            1,
            Mount {
                source: git_canon,
                target: project_target.join(".git"),
                readwrite: true,
            },
        );
    } else if mount_project && !effective_readonly {
        // User explicitly opted out of read-only for the project mount.
        mounts[0].readwrite = true;
    }

    Ok(RunContext {
        image,
        kind,
        cpu,
        memory,
        disk,
        processes,
        disk_priority,
        memory_enforce,
        network_ingress,
        network_egress,
        exec_uid,
        exec_gid,
        container_uid,
        container_gid,
        project_dir: effective_cwd,
        container_home,
        raw_idmap,
        mounts,
        env_map,
        name,
        ephemeral,
        description,
        instance_uuid,
        network_allow,
        network_dns,
        worktree_enabled,
        effective_readonly,
        security_nesting,
        secure_boot,
        notifications,
    })
}

/// Resolve the git identity for worktree auto-commits.
///
/// Priority: `--worktree-user-name`/`--worktree-user-email` CLI flags >
/// `[run.worktree].user_name`/`user_email` config > host project's git
/// config (`git config user.name` / `git config user.email`).
///
/// Returns an error if no identity can be resolved -- the auto-commit
/// safety net requires a known author.
pub fn resolve_git_identity(
    args: &Args,
    wc: &WorktreeConfig,
    project_dir: &Path,
) -> Result<(String, String)> {
    let user_name = args
        .worktree_user_name
        .clone()
        .or_else(|| wc.user_name.clone())
        .or_else(|| git_config(project_dir, "user.name"))
        .context(
            "worktree auto-commit requires user.name; set it in [run.worktree], \
             --worktree-user-name, or the host git config",
        )?;

    let user_email = args
        .worktree_user_email
        .clone()
        .or_else(|| wc.user_email.clone())
        .or_else(|| git_config(project_dir, "user.email"))
        .context(
            "worktree auto-commit requires user.email; set it in [run.worktree], \
             --worktree-user-email, or the host git config",
        )?;

    Ok((user_name, user_email))
}

/// Read a single value from the project's git config on the host.
/// Returns `None` if the command fails or the value is empty.
fn git_config(project_dir: &Path, key: &str) -> Option<String> {
    let val = Command::new("git")
        .args(["-C"])
        .arg(project_dir)
        .args(["config", key])
        .run_stdout()
        .ok()?;
    if val.is_empty() { None } else { Some(val) }
}

/// Build the POSIX `sh` init script that creates a git worktree inside
/// the container and wraps the user's command.
///
/// When `auto_commit` is false (e.g. `--no-auto-commit`), the safety-net
/// commit block is omitted from the generated script entirely.
///
/// `project_path` is the container-side path where the project is
/// mounted (already shell-escaped by the caller).
///
/// The worktree directory is `~/{worktree_name}` -- unique per instance
/// so concurrent instances don't collide on the worktree metadata key.
/// The directory is cleaned up at the end via `git worktree remove
/// --force` (the branch and commits survive).
pub fn build_worktree_init_script(
    branch: &str,
    auto_commit: bool,
    instance_name: &str,
    worktree_name: &str,
    project_path: &str,
) -> String {
    let escaped_branch = shell_escape(branch);
    let escaped_worktree = shell_escape(worktree_name);
    let commit_msg = shell_escape(&format!("celily: auto-commit {instance_name}"));

    let auto_commit_block = if auto_commit {
        format!(
            r#"
if git status --porcelain | grep -q .; then
    git add -A
    git commit --no-verify -m {commit_msg}
fi
"#
        )
    } else {
        String::new()
    };

    format!(
        r#"set -eu
cd {project_path}

branch={escaped_branch}
if git show-ref --verify --quiet "refs/heads/$branch"; then
    git worktree add ~/{escaped_worktree} "$branch"
else
    git worktree add ~/{escaped_worktree} -b "$branch" HEAD
fi
cd ~/{escaped_worktree}

"$@"
rc=$?
{auto_commit_block}
# Remove worktree metadata so {project_path}/.git is clean.
# The branch and all commits survive.
cd {project_path}
git worktree remove --force ~/{escaped_worktree}

exit $rc
"#
    )
}

/// Execute the user's command inside the container via the worktree init
/// script.
///
/// Wraps the command in `sh -c '<init script>' -- <args...>`. Git
/// identity and proxy env vars must already be in `ctx.env_map`.
#[cfg(test)]
mod tests {
    use super::*;

    fn default_args() -> Args {
        Args {
            image: None,
            name: None,
            keep_instance: false,
            vm: false,
            cli_mounts: vec![],
            cpu: None,
            memory: None,
            disk: None,
            processes: None,
            user: None,
            root: false,
            container_uid: None,
            container_gid: None,
            cli_env: vec![],
            cli_pass_env: vec![],
            profile: None,
            no_profile: false,
            cmd_args: vec![],
            worktree: None,
            no_auto_commit: false,
            worktree_user_name: None,
            worktree_user_email: None,
            project_readonly: false,
            no_project_readonly: false,
            mount_project: false,
            no_mount_project: false,
            project_target: None,
        }
    }

    // --- Worktree init script ---

    #[test]
    fn init_script_with_auto_commit() {
        let script = build_worktree_init_script(
            "celily/test-instance",
            true,
            "test-instance",
            "test-instance",
            "'~/work/project'",
        );
        // Should include the auto-commit block.
        assert!(script.contains("git commit --no-verify"));
        assert!(script.contains("celily: auto-commit test-instance"));
        // Should contain worktree creation in a unique directory (shell-escaped).
        assert!(script.contains("git worktree add ~/'test-instance'"));
        // Should use "$@" for command forwarding.
        assert!(script.contains("\"$@\""));
        // Should clean up worktree metadata.
        assert!(script.contains("git worktree remove --force ~/'test-instance'"));
        // Should contain the project path.
        assert!(script.contains("cd '~/work/project'"));
    }

    #[test]
    fn init_script_without_auto_commit() {
        let script = build_worktree_init_script(
            "celily/test-instance",
            false,
            "test-instance",
            "my-worktree",
            "'~/project'",
        );
        // Should NOT include the auto-commit block.
        assert!(!script.contains("git commit --no-verify"));
        assert!(!script.contains("celily: auto-commit"));
        // Should still have the worktree creation.
        assert!(script.contains("git worktree add ~/'my-worktree'"));
        // Should still clean up.
        assert!(script.contains("git worktree remove --force ~/'my-worktree'"));
    }

    #[test]
    fn init_script_escapes_special_chars() {
        // Branch name with a single-quote should be shell-escaped.
        let script = build_worktree_init_script("celily/it's-fine", true, "instance", "it's-fine", "'~/project'");
        // The branch name should be single-quoted.
        assert!(script.contains("'celily/it'"));
    }
}
