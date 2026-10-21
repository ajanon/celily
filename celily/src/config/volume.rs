use std::path::PathBuf;

use celily_lib::AccessMode;
use serde::Deserialize;

/// An existing custom storage volume to attach to the instance.
///
/// Storage that outlives the instance without being on the host
/// filesystem: the volume lives in the backend's pool, with its own size
/// and snapshots. Creating and deleting it is up to the user.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    /// Name of the volume in `[backend] pool`.
    pub name: String,
    /// Mount point inside the instance. A leading `~` expands to the
    /// container home directory.
    pub target: PathBuf,
    #[serde(default)]
    pub access: AccessMode,
}
