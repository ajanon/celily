use merge::Merge;
use serde::Deserialize;

/// Worktree configuration under `[worktree]`.
///
/// All fields are optional. Worktree mode is activated by passing
/// `--worktree NAME` on the CLI; this section configures its behaviour.
#[derive(Debug, Default, Deserialize, Merge)]
#[serde(default)]
#[merge(strategy = super::merge_strategy::overwrite_some)]
pub struct WorktreeConfig {
    /// Template for the worktree branch name. `{name}` is replaced with
    /// the name given to `--worktree`. Defaults to `"celily/{name}"`
    /// when `None`.
    pub branch: Option<String>,

    /// Auto-commit uncommitted changes before container teardown.
    /// Defaults to `true` when `None`.
    #[serde(default)]
    pub auto_commit: Option<bool>,

    /// Git user.name for auto-commits. Falls back to the host project's
    /// git config when `None`.
    pub user_name: Option<String>,

    /// Git user.email for auto-commits. Falls back to the host project's
    /// git config when `None`.
    pub user_email: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_empty_section() {
        let cfg: WorktreeConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.branch, None);
        assert_eq!(cfg.auto_commit, None);
        assert_eq!(cfg.user_name, None);
        assert_eq!(cfg.user_email, None);
    }

    #[test]
    fn deserialize_full() {
        let cfg: WorktreeConfig = toml::from_str(
            r#"
            branch = "agents/{name}"
            auto_commit = false
            user_name = "Agent"
            user_email = "agent@example.com"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.branch.as_deref(), Some("agents/{name}"));
        assert_eq!(cfg.auto_commit, Some(false));
        assert_eq!(cfg.user_name.as_deref(), Some("Agent"));
        assert_eq!(cfg.user_email.as_deref(), Some("agent@example.com"));
    }
}
