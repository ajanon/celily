mod acl;
mod bridge;
pub mod instance;
pub mod network;

use crate::command::{CommandError, CommandExt};

/// LXC-family backend -- spawns the `lxc` or `incus` CLI for all operations.
///
/// The [`lxd`](Self::lxd) and [`incus`](Self::incus) constructors select the
/// binary. `remote`, `project`, and `pool` scope all invocations and configure
/// the root disk device.
#[derive(Debug, Clone)]
pub struct LxcBackend {
    pub remote: Option<String>,
    pub project: Option<String>,
    pub pool: String,
    binary: &'static str,
}

impl LxcBackend {
    /// Backend targeting the LXD daemon.
    pub fn lxd() -> Self {
        Self {
            remote: None,
            project: None,
            pool: "default".into(),
            binary: "lxc",
        }
    }

    /// Backend targeting the Incus daemon.
    pub fn incus() -> Self {
        Self {
            remote: None,
            project: None,
            pool: "default".into(),
            binary: "incus",
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
    /// are silently ignored.
    ///
    /// Also ensures the project's default profile has a root disk device
    /// pointing at the configured [`pool`](Self::pool), since instances
    /// require a root disk at creation time.
    pub fn ensure_project(&self) -> Result<(), CommandError> {
        let Some(ref project) = self.project else {
            return Ok(());
        };

        match self
            .lxc_project_command()
            .args([
                "project",
                "create",
                project,
                "--config",
                "features.images=true",
            ])
            .run()
        {
            Ok(()) => {},
            Err(CommandError::NonZero { ref stderr, .. })
                if stderr
                    .as_deref()
                    .is_some_and(|s| s.contains("already exists")) => {},
            Err(e) => return Err(e),
        }

        // Ensure the default profile has a root disk device with the
        // configured pool. Try add first (device doesn't exist yet),
        // then set the pool if it already does (pool may have changed).
        //
        // The profile is otherwise empty -- no bridge NIC, clean.
        let root_res = self
            .lxc_project_command()
            .args([
                "profile",
                "device",
                "add",
                "default",
                "root",
                "disk",
                "path=/",
                &format!("pool={}", self.pool),
            ])
            .run();

        match root_res {
            Ok(()) => {},
            Err(CommandError::NonZero { ref stderr, .. })
                if stderr
                    .as_deref()
                    .is_some_and(|s| s.contains("already exists")) =>
            {
                // Device exists from a previous run -- set the pool
                // to reconcile in case it changed.
                self.lxc_project_command()
                    .args([
                        "profile", "device", "set", "default", "root", "pool", &self.pool,
                    ])
                    .run()?;
            },
            Err(e) => return Err(e),
        }

        Ok(())
    }

    /// Build a command that targets the default project.
    /// Used for network operations (bridge networks are unsupported
    /// in non-default LXD projects).
    pub(super) fn lxc_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(self.binary);
        if let Some(ref remote) = self.remote {
            cmd.args(["--remote", remote]);
        }
        cmd
    }

    /// Build a command scoped to the configured project.
    /// Used for instance and profile operations.
    pub(super) fn lxc_project_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(self.binary);
        if let Some(ref project) = self.project {
            cmd.args(["--project", project]);
        }
        if let Some(ref remote) = self.remote {
            cmd.args(["--remote", remote]);
        }
        cmd
    }
}
