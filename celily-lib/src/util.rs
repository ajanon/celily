use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CleanupPath
// ---------------------------------------------------------------------------

/// A path that is deleted when the value goes out of scope.
///
/// Silently ignores `NotFound` errors - the file may already have been
/// cleaned up by another path. Other errors are logged.
pub struct CleanupPath(PathBuf);

impl CleanupPath {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Drop for CleanupPath {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::error!(
                path = %self.0.display(),
                error = %e,
                "CleanupPath: failed to remove file"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CleanupDir
// ---------------------------------------------------------------------------

/// A directory that is recursively deleted when the value goes out of scope.
///
/// Silently ignores `NotFound` errors - the directory may already have been
/// cleaned up by another path. Other errors are logged.
pub struct CleanupDir(PathBuf);

impl CleanupDir {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Drop for CleanupDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::error!(
                path = %self.0.display(),
                error = %e,
                "CleanupDir: failed to remove directory"
            );
        }
    }
}
