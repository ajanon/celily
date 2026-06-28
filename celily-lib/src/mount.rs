use std::path::PathBuf;

use serde::Deserialize;

/// Access mode for a bind-mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, strum::EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum AccessMode {
    /// Read-only access (the default).
    #[default]
    ReadOnly,
    /// Read-write access.
    ReadWrite,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mount {
    pub source: PathBuf,
    pub target: PathBuf,
    #[serde(default)]
    pub access: AccessMode,
}
