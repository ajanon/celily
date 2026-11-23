mod acl;
mod bridge;
pub mod instance;
pub mod network;
mod payload;

use crate::command::{AsyncCommandExt, CommandError};

/// LXC-family backend -- spawns the `lxc` or `incus` CLI for all operations.
///
/// The [`lxd`](Self::lxd) and [`incus`](Self::incus) constructors select the
/// daemon. `remote` and `project` scope all invocations, `pool` holds the
/// root disk.
#[derive(Debug, Clone)]
pub struct LxcBackend {
    pub remote: Option<String>,
    pub project: Option<String>,
    pub pool: String,
    daemon: Daemon,
}

/// The daemon behind the CLI: everything that differs between LXD and Incus.
#[derive(Debug, Clone, Copy)]
enum Daemon {
    Lxd,
    Incus,
}

impl Daemon {
    fn binary(self) -> &'static str {
        match self {
            Self::Lxd => "lxc",
            Self::Incus => "incus",
        }
    }

    /// The instance key that hides the guest API (`/dev/lxd`,
    /// `/dev/incus`). Incus renamed LXD's key and rejects the old name.
    fn guest_api_key(self) -> &'static str {
        match self {
            Self::Lxd => "security.devlxd",
            Self::Incus => "security.guestapi",
        }
    }

    /// The instance config entry that turns UEFI secure boot off on VMs.
    fn no_secure_boot(self) -> (&'static str, &'static str) {
        match self {
            Self::Lxd => ("boot.mode", "uefi-nosecureboot"),
            Self::Incus => ("security.secureboot", "false"),
        }
    }
}

impl LxcBackend {
    /// Backend targeting the LXD daemon.
    pub fn lxd() -> Self {
        Self {
            remote: None,
            project: None,
            pool: "default".into(),
            daemon: Daemon::Lxd,
        }
    }

    /// Backend targeting the Incus daemon.
    pub fn incus() -> Self {
        Self {
            remote: None,
            project: None,
            pool: "default".into(),
            daemon: Daemon::Incus,
        }
    }

    /// Convenience alias for [`lxd`](Self::lxd).
    pub fn new() -> Self {
        Self::lxd()
    }

    /// Ensure the configured LXD project exists, creating it if necessary.
    ///
    /// Does nothing when [`project`](Self::project) is `None`.
    /// Idempotent -- "already exists" errors from `lxc project create`
    /// are silently ignored. Its profiles are never used: instances are
    /// created without any.
    pub async fn ensure_project(&self) -> Result<(), CommandError> {
        let Some(ref project) = self.project else {
            return Ok(());
        };

        match self
            .lxc_project_command_async()
            .args([
                "project",
                "create",
                project,
                "--config",
                "features.images=true",
            ])
            .run()
            .await
        {
            Ok(()) => Ok(()),
            Err(CommandError::NonZero { ref stderr, .. })
                if stderr
                    .as_deref()
                    .is_some_and(|s| s.contains("already exists")) =>
            {
                Ok(())
            },
            Err(e) => Err(e),
        }
    }

    /// Build a command that targets the default project.
    /// Used for network operations (bridge networks are unsupported
    /// in non-default LXD projects).
    pub(super) fn lxc_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(self.daemon.binary());
        if let Some(ref remote) = self.remote {
            cmd.args(["--remote", remote]);
        }
        cmd
    }

    /// Build a command scoped to the configured project.
    /// Used for instance and profile operations.
    pub(super) fn lxc_project_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(self.daemon.binary());
        if let Some(ref project) = self.project {
            cmd.args(["--project", project]);
        }
        if let Some(ref remote) = self.remote {
            cmd.args(["--remote", remote]);
        }
        cmd
    }

    /// Async variant of [`lxc_command`](Self::lxc_command).
    pub(super) fn lxc_command_async(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(self.daemon.binary());
        if let Some(ref remote) = self.remote {
            cmd.args(["--remote", remote]);
        }
        cmd
    }

    /// Async variant of [`lxc_project_command`](Self::lxc_project_command).
    pub(super) fn lxc_project_command_async(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(self.daemon.binary());
        if let Some(ref project) = self.project {
            cmd.args(["--project", project]);
        }
        if let Some(ref remote) = self.remote {
            cmd.args(["--remote", remote]);
        }
        cmd
    }
}
