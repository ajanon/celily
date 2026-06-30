use std::path::PathBuf;

/// Access mode for a bind-mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::EnumString)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[strum(serialize_all = "lowercase")]
pub enum AccessMode {
    /// Read-only access (the default).
    #[default]
    ReadOnly,
    /// Read-write access.
    ReadWrite,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
pub struct Mount {
    pub source: PathBuf,
    pub target: PathBuf,
    #[cfg_attr(feature = "serde", serde(default))]
    pub access: AccessMode,
}
