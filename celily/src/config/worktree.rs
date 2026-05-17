use serde::Deserialize;

/// Worktree configuration under `[worktree]`.
///
/// All fields are optional. Worktree mode is activated by passing
/// `--worktree NAME` on the CLI; this section configures its behaviour.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
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

impl WorktreeConfig {
    /// Merge two `WorktreeConfig` values. Profile scalars override
    /// default; `None` means "not set, fall back."
    pub(super) fn merge(default: Self, profile: Self) -> Self {
        Self {
            branch: profile.branch.or(default.branch),
            auto_commit: profile.auto_commit.or(default.auto_commit),
            user_name: profile.user_name.or(default.user_name),
            user_email: profile.user_email.or(default.user_email),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_override() {
        let mut default = WorktreeConfig::default();
        default.branch = Some("celily/{name}".into());
        let mut profile = WorktreeConfig::default();
        profile.branch = Some("agents/{name}".into());
        let merged = WorktreeConfig::merge(default, profile);
        assert_eq!(merged.branch.as_deref(), Some("agents/{name}"));
    }

    #[test]
    fn branch_fallback() {
        let mut default = WorktreeConfig::default();
        default.branch = Some("celily/{name}".into());
        let profile = WorktreeConfig::default();
        let merged = WorktreeConfig::merge(default, profile);
        assert_eq!(merged.branch.as_deref(), Some("celily/{name}"));
    }

    #[test]
    fn auto_commit_explicit_false() {
        let mut default = WorktreeConfig::default();
        default.auto_commit = Some(true);
        let mut profile = WorktreeConfig::default();
        profile.auto_commit = Some(false);
        let merged = WorktreeConfig::merge(default, profile);
        assert_eq!(merged.auto_commit, Some(false));
    }

    #[test]
    fn auto_commit_fallback() {
        let mut default = WorktreeConfig::default();
        default.auto_commit = Some(true);
        let profile = WorktreeConfig::default();
        let merged = WorktreeConfig::merge(default, profile);
        assert_eq!(merged.auto_commit, Some(true));
    }

    #[test]
    fn user_name_profile_wins() {
        let mut default = WorktreeConfig::default();
        default.user_name = Some("Default Agent".into());
        let mut profile = WorktreeConfig::default();
        profile.user_name = Some("Profile Agent".into());
        let merged = WorktreeConfig::merge(default, profile);
        assert_eq!(merged.user_name.as_deref(), Some("Profile Agent"));
    }

    #[test]
    fn user_email_fallback() {
        let mut default = WorktreeConfig::default();
        default.user_email = Some("default@example.com".into());
        let profile = WorktreeConfig::default();
        let merged = WorktreeConfig::merge(default, profile);
        assert_eq!(merged.user_email.as_deref(), Some("default@example.com"));
    }

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
