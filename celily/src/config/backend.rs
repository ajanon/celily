use serde::Deserialize;

/// Which backend to use for managing instances.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Lxd,
    Incus,
}

/// Backend selection configuration.
#[derive(Debug, Deserialize)]
pub struct BackendConfig {
    /// Which backend to use. Currently `lxd` and `incus` are supported.
    #[serde(default)]
    pub kind: Option<BackendKind>,
    /// LXD project name. When set, all LXD commands are scoped to this
    /// project (via `--project`). When `None`, the daemon's default
    /// project is used. Optional.
    #[serde(default)]
    pub project: Option<String>,
    /// Storage pool for the root disk device. Applies to both containers
    /// and VMs. When `None`, falls back to `"default"` at runtime.
    #[serde(default)]
    pub pool: Option<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            kind: None,
            project: None,
            pool: None,
        }
    }
}

impl BackendConfig {
    /// Merge a profile config over the default.
    pub fn merge(default: &Self, profile: &Self) -> Self {
        Self {
            kind: profile.kind.clone().or(default.kind.clone()),
            project: profile.project.clone().or(default.project.clone()),
            pool: profile.pool.clone().or(default.pool.clone()),
        }
    }
}
