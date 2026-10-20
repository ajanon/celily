use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use celily_lib::{AccessMode, Device, InstanceKind, Mount, NetworkRule};

use crate::cli::Args;
use crate::config::{Config, WorktreeConfig};
use crate::util::{
    expand_container_tilde,
    expand_host_tilde,
    git_config,
    instance_name,
    is_valid_username,
};
use crate::validate::{
    Forbidden,
    canonicalize_in_home,
    validate_mount_source,
    validate_proxy_connect,
};

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
    /// Enable security.nesting (required for snapd, Docker, etc.).
    pub security_nesting: bool,
    /// Disable UEFI secure boot for VMs. Ignored on containers.
    /// Default: true (LXD default).
    pub secure_boot: bool,
    /// Pre-built extra devices (proxies, etc.) from config.
    pub extra_devices: Vec<Device>,
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

    let container_uid = args.container_uid.or(cfg.container_uid).unwrap_or(1000);
    let container_gid = args.container_gid.or(cfg.container_gid).unwrap_or(1000);
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

    // Worktree mode never mounts .git read-write: the sandbox could plant
    // hooks/config that run on the host. Commits leave via git_remote.rs.
    let project_readwrite = !worktree_enabled
        && (args.project_readwrite
            || (!args.no_project_readwrite && cfg.project_readwrite.unwrap_or(false)));

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
            access: if project_readwrite {
                AccessMode::ReadWrite
            } else {
                AccessMode::ReadOnly
            },
        });
    }

    mounts.extend(cfg.mounts.iter().map(|m| Mount {
        source: expand_host_tilde(&m.source, home),
        target: expand_container_tilde(&m.target, &container_home),
        access: m.access,
    }));
    mounts.extend(args.cli_mounts.iter().map(|m| Mount {
        source: expand_host_tilde(&m.source, home),
        target: expand_container_tilde(&m.target, &container_home),
        access: m.access,
    }));

    // Pre-canonicalize allowlists once so validate_mount_source doesn't
    // re-canonicalize them for every mount. Missing entries are skipped -
    // mounting them fails anyway. An entry going through a symlink under
    // $HOME is an error, not a skip: see canonicalize_in_home.
    let canonicalize_allowlist = |key: &str, entries: &[PathBuf]| -> Result<Vec<PathBuf>> {
        let mut canonical = Vec::new();
        for entry in entries {
            let entry = expand_host_tilde(entry, home);
            if !entry.exists() {
                tracing::warn!(
                    "{key} entry '{}' skipped: missing or inaccessible",
                    entry.display(),
                );
                continue;
            }
            canonical.push(
                canonicalize_in_home(&entry, home_canon)
                    .with_context(|| format!("invalid {key} entry"))?,
            );
        }
        Ok(canonical)
    };
    let canon_allowed_dirs = canonicalize_allowlist("allowed_dirs", allowed_dirs)?;
    let canon_allowed_files = canonicalize_allowlist("allowed_files", allowed_files)?;

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
    let security_nesting = cfg.security_nesting.unwrap_or(false);
    let secure_boot = cfg.secure_boot.unwrap_or(true);

    // Validate proxy devices and convert to backend Device::Proxy.
    let host_uid = nix::unistd::Uid::current().as_raw();
    let host_gid = nix::unistd::Gid::current().as_raw();
    let mut extra_devices: Vec<Device> = Vec::new();
    for proxy in &cfg.proxy {
        validate_proxy_connect(&proxy.connect, home_canon, host_uid)?;
        extra_devices.push(Device::Proxy {
            connect: proxy.connect.clone(),
            listen: proxy.listen.clone(),
            uid: container_uid,
            gid: container_gid,
            host_uid,
            host_gid,
            mode: proxy.mode.clone(),
        });
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
        security_nesting,
        secure_boot,
        extra_devices,
    })
}

/// Resolve the git identity for commits made in worktree mode.
///
/// Priority: `--worktree-user-name`/`--worktree-user-email` CLI flags >
/// `[worktree].user_name`/`user_email` config > host project's git
/// config (`git config user.name` / `git config user.email`).
///
/// Returns an error if no identity can be resolved.
pub async fn resolve_git_identity(
    args: &Args,
    wc: &WorktreeConfig,
    project_dir: &Path,
) -> Result<(String, String)> {
    let user_name = if let Some(name) = args.worktree_user_name.clone() {
        Some(name)
    } else if let Some(name) = wc.user_name.clone() {
        Some(name)
    } else {
        git_config(project_dir, &["user.name"]).await
    }
    .context(
        "worktree mode requires user.name; set it in [worktree], --worktree-user-name, or the \
         host git config",
    )?;

    let user_email = if let Some(email) = args.worktree_user_email.clone() {
        Some(email)
    } else if let Some(email) = wc.user_email.clone() {
        Some(email)
    } else {
        git_config(project_dir, &["user.email"]).await
    }
    .context(
        "worktree mode requires user.email; set it in [worktree], --worktree-user-email, or the \
         host git config",
    )?;

    Ok((user_name, user_email))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    /// Read-only by default, and always in worktree mode.
    #[test]
    fn project_mount_access() {
        use AccessMode::{ReadOnly, ReadWrite};

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let (project, config_dir) = (home.join("project"), home.join(".config"));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        let access = |cli: &[&str], readwrite: Option<bool>| {
            let args =
                Args::try_parse_from([&["celily", "--mount-project"], cli].concat()).unwrap();
            let cfg = Config {
                project_readwrite: readwrite,
                ..Config::default()
            };
            let ctx = resolve_context(
                &args,
                &cfg,
                &project,
                &home,
                &home,
                &config_dir,
                std::slice::from_ref(&project),
                &[],
            )
            .unwrap();
            ctx.mounts[0].access
        };
        assert_eq!(access(&[], None), ReadOnly);
        assert_eq!(access(&[], Some(false)), ReadOnly);
        assert_eq!(access(&[], Some(true)), ReadWrite);
        assert_eq!(access(&["--project-readwrite"], None), ReadWrite);
        assert_eq!(access(&["--no-project-readwrite"], Some(true)), ReadOnly);
        assert_eq!(access(&["-w", "foo"], Some(true)), ReadOnly);
    }
}
