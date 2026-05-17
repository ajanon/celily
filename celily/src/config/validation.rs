use std::path::Path;

use super::ConfigError;

/// Require that `path` is owned by the current user and has no group/other
/// permission bits set (mode `0o0077`). Security boundary - prevents loading
/// config from files writable by others.
pub(super) fn validate_node_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::linux::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let uid = meta.st_uid();
    let mode = meta.permissions().mode();
    let euid = nix::unistd::Uid::current().as_raw();
    if uid != euid {
        return Err(ConfigError::Validation(format!(
            "{} is owned by uid {uid} (expected {euid}); refusing to load",
            path.display(),
        )));
    }
    if mode & 0o0077 != 0 {
        return Err(ConfigError::Validation(format!(
            "{} has mode {mode:o}; group/other permissions must not be set",
            path.display(),
        )));
    }
    Ok(())
}

/// Verify that the celily config directory and profiles directory (if they
/// exist) are owned by the current user and have no group/other permissions
/// set.
pub(super) fn validate_config_dirs() -> Result<(), ConfigError> {
    let dir = super::config_dir().join("celily");
    if dir.exists() {
        validate_node_permissions(&dir)?;
    }
    let profiles_dir = dir.join("profiles");
    if profiles_dir.exists() {
        validate_node_permissions(&profiles_dir)?;
    }
    Ok(())
}
