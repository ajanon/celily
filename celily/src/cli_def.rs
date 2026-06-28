use std::io;
use std::path::PathBuf;

use clap::Parser;
use clap_complete::Shell;
use celily_lib::{AccessMode, Mount};

#[derive(Parser)]
#[command(about = "Launch an ephemeral LXD container for running commands")]
pub struct Args {
    /// Image name to launch [default from config, then "celily"]
    #[arg(long)]
    pub image: Option<String>,

    /// Override the generated instance name
    #[arg(long)]
    pub name: Option<String>,

    /// Keep the instance after exit (implies non-ephemeral)
    #[arg(long)]
    pub keep_instance: bool,

    /// Run as a VM instead of a container [default from config, then false]
    #[arg(long)]
    pub vm: bool,

    /// Mount a host directory into the container: SOURCE:TARGET[:readwrite|readonly]
    #[arg(long = "mount", value_name = "SOURCE:TARGET[:readwrite|readonly]", value_parser = parse_cli_mount)]
    pub cli_mounts: Vec<Mount>,

    /// CPU limit [default from limits.cpu, then 2]
    #[arg(long)]
    pub cpu: Option<u32>,

    /// Memory limit, e.g. 4GiB [default from limits.memory, then "4GiB"]
    #[arg(long)]
    pub memory: Option<String>,

    /// Root disk size, e.g. 4GiB [default from limits.disk, then "4GiB"]
    #[arg(long)]
    pub disk: Option<String>,

    /// Maximum number of processes, -1 for unlimited [default from
    /// limits.processes, then 1024]
    #[arg(long)]
    pub processes: Option<i32>,

    /// User to exec as inside the container [default from config, then "dev"]
    #[arg(long)]
    pub user: Option<String>,

    /// Exec as root (uid 0) instead of the configured user
    #[arg(long)]
    pub root: bool,

    /// UID of the non-root user inside the container, used for idmaps [default
    /// from config, then 1000]
    #[arg(long)]
    pub container_uid: Option<u32>,

    /// GID of the non-root user's primary group inside the container, used for
    /// idmaps [default from config, then 1000]
    #[arg(long)]
    pub container_gid: Option<u32>,

    /// Set an environment variable in the container (repeatable, overrides
    /// config)
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub cli_env: Vec<String>,

    /// Forward a host environment variable into the container (repeatable)
    #[arg(long = "pass-env", value_name = "NAME")]
    pub cli_pass_env: Vec<String>,

    /// Target path for the project bind-mount inside the container (tilde expands to
    /// container home) [default: ~/project]
    #[arg(long = "project-target", value_name = "PATH")]
    pub project_target: Option<String>,

    /// Use the named profile (merges ~/.config/celily/profiles/<profile>.toml
    /// over default config)
    #[arg(long = "profile", value_name = "NAME")]
    pub profile: Option<String>,

    /// Skip profile auto-detection; use only the default config
    #[arg(long, conflicts_with = "profile")]
    pub no_profile: bool,

    /// Arguments to run inside the container (pass after --)
    #[arg(last = true, allow_hyphen_values = true)]
    pub cmd_args: Vec<String>,

    /// Enable worktree mode with the given name. Replaces {name} in
    /// the branch template (default: "celily/{name}").
    #[arg(short = 'w', long, value_name = "NAME")]
    pub worktree: Option<String>,

    /// Disable the safety-net auto-commit for this run
    #[arg(long)]
    pub no_auto_commit: bool,

    /// Override git user.name for auto-commits
    #[arg(long, value_name = "NAME")]
    pub worktree_user_name: Option<String>,

    /// Override git user.email for auto-commits
    #[arg(long, value_name = "EMAIL")]
    pub worktree_user_email: Option<String>,

    /// Mount the project directory into the container (opt-in; default is no
    /// mount). Ignored in worktree mode, which always mounts the project
    /// directory.
    #[arg(long, conflicts_with = "no_mount_project")]
    pub mount_project: bool,

    /// Explicitly disable project directory mount (overrides config).
    /// Conflicts with --mount-project and --worktree.
    #[arg(long, conflicts_with = "mount_project", conflicts_with = "worktree")]
    pub no_mount_project: bool,

    /// Mount the project directory read-only for this invocation
    #[arg(long)]
    pub project_readonly: bool,

    /// Explicitly disable read-only project mount (overrides config)
    #[arg(long, conflicts_with = "project_readonly", conflicts_with = "worktree")]
    pub no_project_readonly: bool,
}

/// Generate shell completions and write them to `out`.
#[allow(dead_code)]
pub fn generate_completions(shell: Shell, out: &mut dyn io::Write) {
    let mut cmd = <Args as clap::CommandFactory>::command();
    let name = cmd.get_name().to_owned();
    clap_complete::generate(shell, &mut cmd, &name, out);
}

/// Parse a `--mount` argument of the form `SOURCE:TARGET[:readwrite|readonly]`.
pub fn parse_cli_mount(s: &str) -> Result<Mount, String> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(format!("expected 'source:target[:readwrite|readonly]', got '{s}'"));
    }
    let access = match parts.get(2) {
        None => AccessMode::ReadOnly,
        Some(&flag) => flag.parse().map_err(|e| format!("invalid mount flag: {e}"))?,
    };
    Ok(Mount {
        source: PathBuf::from(parts[0]),
        target: PathBuf::from(parts[1]),
        access,
    })
}
