use std::path::PathBuf;

/// Access mode for a bind-mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::EnumString, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum AccessMode {
    /// Read-only access (the default).
    #[default]
    ReadOnly,
    /// Read-write access.
    ReadWrite,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    pub source: PathBuf,
    pub target: PathBuf,
    #[serde(default)]
    pub access: AccessMode,
}
