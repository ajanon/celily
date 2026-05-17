use std::path::{Path, PathBuf};

/// Expand a leading `~` in a host path to `home`.
///
/// Only `~` alone or `~/...` are expanded. Paths like `~user/...` are
/// returned unchanged -- resolving another user's home directory is outside
/// the scope of this function (standard POSIX behaviour for non-shell
/// tilde expansion).
///
/// Paths without a leading `~` are returned unchanged.
#[must_use]
pub fn expand_host_tilde(path: &Path, home: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return home.join(rest);
    }
    path.to_path_buf()
}

/// Expand a leading `~` in a container path to the container's home directory.
///
/// Only `~` alone or `~/...` are expanded. Paths like `~user/...` are
/// returned unchanged.
///
/// Paths without a leading `~` are returned unchanged.
#[must_use]
pub fn expand_container_tilde(path: &Path, container_home: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" {
        return container_home.to_path_buf();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return container_home.join(rest);
    }
    path.to_path_buf()
}

/// Return true if `path` is equal to `parent` or is a proper descendant of it
/// (i.e. `path.starts_with(parent)` and the remainder after stripping the
/// parent is non-empty).
///
/// Does not resolve symlinks - canonical paths should be used if symlinks
/// are a concern.
pub fn is_under_or_eq(path: &Path, parent: &Path) -> bool {
    path == parent
        || path.starts_with(parent)
            && path
                .strip_prefix(parent)
                .is_ok_and(|r| !r.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- expand_host_tilde / expand_container_tilde ---

    #[test]
    fn expand_host_tilde_leading_tilde() {
        let result = expand_host_tilde(Path::new("~/projects/foo"), Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("/home/user/projects/foo"));
    }

    #[test]
    fn expand_host_tilde_tilde_only() {
        let result = expand_host_tilde(Path::new("~"), Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("/home/user"));
    }

    #[test]
    fn expand_host_tilde_absolute_path_unchanged() {
        let result = expand_host_tilde(Path::new("/etc/passwd"), Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn expand_host_tilde_relative_path_unchanged() {
        let result = expand_host_tilde(Path::new("foo/bar"), Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("foo/bar"));
    }

    #[test]
    fn expand_host_tilde_tilde_slash() {
        let result = expand_host_tilde(Path::new("~/"), Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("/home/user"));
    }

    #[test]
    fn expand_host_tilde_tilde_user_rejected() {
        let result = expand_host_tilde(Path::new("~root/.ssh"), Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("~root/.ssh"));
    }

    #[test]
    fn expand_host_tilde_tilde_user_only_rejected() {
        let result = expand_host_tilde(Path::new("~root"), Path::new("/home/user"));
        assert_eq!(result, PathBuf::from("~root"));
    }

    #[test]
    fn expand_container_tilde_basic() {
        let result = expand_container_tilde(Path::new("~/work"), Path::new("/home/dev"));
        assert_eq!(result, PathBuf::from("/home/dev/work"));
    }

    #[test]
    fn expand_container_tilde_no_tilde() {
        let result = expand_container_tilde(Path::new("/opt/app"), Path::new("/home/dev"));
        assert_eq!(result, PathBuf::from("/opt/app"));
    }

    #[test]
    fn expand_container_tilde_user_rejected() {
        let result = expand_container_tilde(Path::new("~root/.config"), Path::new("/home/dev"));
        assert_eq!(result, PathBuf::from("~root/.config"));
    }

    // --- is_under_or_eq ---

    #[test]
    fn is_under_or_eq_exact_match() {
        assert!(is_under_or_eq(Path::new("/a"), Path::new("/a")));
    }

    #[test]
    fn is_under_or_eq_child() {
        assert!(is_under_or_eq(Path::new("/a/b"), Path::new("/a")));
    }

    #[test]
    fn is_under_or_eq_deep_child() {
        assert!(is_under_or_eq(Path::new("/a/b/c/d"), Path::new("/a")));
    }

    #[test]
    fn is_under_or_eq_sibling_not_under() {
        assert!(!is_under_or_eq(Path::new("/a/foo"), Path::new("/a/bar")));
    }

    #[test]
    fn is_under_or_eq_prefix_false_positive() {
        // /ab should NOT match as under /a
        assert!(!is_under_or_eq(Path::new("/ab"), Path::new("/a")));
    }

    #[test]
    fn is_under_or_eq_parent_not_match() {
        assert!(!is_under_or_eq(Path::new("/a"), Path::new("/a/b")));
    }

    #[test]
    fn is_under_or_eq_root() {
        assert!(is_under_or_eq(Path::new("/"), Path::new("/")));
        assert!(!is_under_or_eq(Path::new("/"), Path::new("/a")));
    }

    #[test]
    fn is_under_or_eq_empty_paths() {
        assert!(!is_under_or_eq(Path::new(""), Path::new("/a")));
    }
}
