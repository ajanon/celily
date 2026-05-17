pub mod backend;
pub mod limits;
pub mod network;
pub mod validation;
pub mod worktree;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

pub use self::backend::{BackendConfig, BackendKind};
pub use self::limits::Limits;
pub use self::network::NetworkConfig;
pub use self::worktree::WorktreeConfig;
use celily_lib::DistroKind;
use self::validation::{validate_config_dirs, validate_node_permissions};
use celily_lib::InstanceKind;
use celily_lib::Mount;
use crate::util::{expand_host_tilde, is_under_or_eq};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("{0}")]
    Validation(String),
}

fn default_notifications() -> Option<bool> {
    Some(true)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    // -- Backend selection --
    pub backend: BackendConfig,

    // -- Instance identity / infrastructure --
    pub image: Option<String>,
    /// Run as a VM instead of a container.
    #[serde(rename = "vm")]
    pub kind: Option<InstanceKind>,
    /// Non-root user created inside the container.
    pub user: Option<String>,
    /// UID assigned to `user` inside the container; used for idmaps.
    pub container_uid: Option<u32>,
    /// GID assigned to `user`'s primary group inside the container; used for
    /// idmaps.
    pub container_gid: Option<u32>,
    /// Which distribution the image is based on. Determines distro-specific
    /// behaviour.
    pub distro: Option<DistroKind>,
    /// Enable LXD security.nesting (required for snapd, Docker, etc.).
    pub security_nesting: Option<bool>,
    /// Disable UEFI secure boot for VMs. Ignored on containers.
    pub secure_boot: Option<bool>,

    // -- Runtime behaviour --
    pub limits: Limits,
    pub mounts: Vec<Mount>,
    /// Absolute directory paths that may be bind-mounted. Must be an exact
    /// match (not a subtree).
    pub allowed_dirs: Vec<PathBuf>,
    /// Absolute file paths that may be bind-mounted. Must be an exact match.
    pub allowed_files: Vec<PathBuf>,
    /// Static environment variables set for every `lxc exec` inside the
    /// container. CLI `--env` overrides take precedence.
    pub env: HashMap<String, String>,
    /// Target path inside the container for the project bind-mount. Tilde is
    /// expanded to the container home directory. Default: `~/project`.
    pub project_target: Option<String>,
    /// Names of host environment variables to forward into the container.
    /// Overrides static `env` entries for the same key.
    pub pass_env: Vec<String>,
    /// Name of the active secret provider. Available providers are listed
    /// in `celily-config`(5). When absent, secrets are not resolved;
    /// attempting to use `auth.secret` in a network allow rule without a
    /// configured provider is an error.
    pub secret_provider: Option<String>,
    /// Network isolation configuration. When enabled, the instance gets a
    /// dedicated bridge with an egress ACL and a per-instance mitmproxy.
    pub network: NetworkConfig,
    /// Mount the project directory read-only. When `None`, the effective
    /// default depends on worktree mode: `true` when worktree is enabled,
    /// `false` otherwise. `Some(true)` or `Some(false)` overrides the
    /// default regardless of worktree mode.
    pub project_readonly: Option<bool>,
    /// Worktree configuration. Worktree mode is activated by passing
    /// `--worktree NAME` on the CLI; this section configures its
    /// behaviour. See `celily`(1) for details.
    pub worktree: WorktreeConfig,
    /// Mount the project directory (current working directory) into the
    /// container. When `None`, the project directory is not mounted unless
    /// worktree mode is active (which always mounts it). Set to `true` to
    /// opt in, `false` to explicitly disable.
    pub mount_project: Option<bool>,
    /// Inline script run as the container user before the main command
    /// (or `bash --login`). Shebang-aware: written to a temp file and
    /// executed, so `#!/usr/bin/env python3` works. The same
    /// environment variables as the main command are set, plus
    /// `CELILY_USER`, `CELILY_UID`, `CELILY_GID`, `CELILY_HOME`.
    /// Empty string or whitespace-only is treated as not set.
    pub pre_run: Option<String>,
    /// Whether to bind-mount the notification proxy socket into the
    /// container. When true (default), the host's
    /// `xdg-dbus-proxy-notifications.service` socket is exposed at
    /// `/run/dbus-notifications.sock` inside the container, allowing
    /// tools like `notify-send` to send desktop notifications through
    /// the host's session bus. Set to false to disable. Ignored on VMs.
    #[serde(default = "default_notifications")]
    pub notifications: Option<bool>,
}

/// Parsed representation of `profiles.toml`.
#[derive(Deserialize)]
struct ProfilesToml {
    profiles: HashMap<String, String>,
    #[serde(default)]
    inherit: HashMap<String, String>,
}

impl Config {
    /// Merge two `Config` values. Scalars: profile wins when set.
    /// Lists: concatenated (default first, profile appended).
    /// Maps: merged with profile keys overriding.
    pub(super) fn merge(default: Self, profile: Self) -> Self {
        Self {
            backend: BackendConfig::merge(&default.backend, &profile.backend),
            image: profile.image.or(default.image),
            kind: profile.kind.or(default.kind),
            user: profile.user.or(default.user),
            container_uid: profile.container_uid.or(default.container_uid),
            container_gid: profile.container_gid.or(default.container_gid),
            distro: profile.distro.or(default.distro),
            security_nesting: profile.security_nesting.or(default.security_nesting),
            secure_boot: profile.secure_boot.or(default.secure_boot),

            limits: Limits::merge(&default.limits, &profile.limits),
            mounts: {
                let mut mounts = default.mounts;
                mounts.extend(profile.mounts);
                mounts
            },
            allowed_dirs: {
                let mut dirs = default.allowed_dirs;
                dirs.extend(profile.allowed_dirs);
                dirs
            },
            allowed_files: {
                let mut files = default.allowed_files;
                files.extend(profile.allowed_files);
                files
            },
            env: {
                let mut env = default.env;
                env.extend(profile.env);
                env
            },
            pass_env: {
                let mut pe = default.pass_env;
                pe.extend(profile.pass_env);
                pe
            },
            secret_provider: profile.secret_provider.or(default.secret_provider),
            project_target: profile.project_target.or(default.project_target),
            network: NetworkConfig::merge(default.network, profile.network),
            project_readonly: profile.project_readonly.or(default.project_readonly),
            mount_project: profile.mount_project.or(default.mount_project),
            worktree: WorktreeConfig::merge(default.worktree, profile.worktree),
            pre_run: profile.pre_run.or(default.pre_run),
            notifications: profile.notifications.or(default.notifications),
        }
    }

    /// Load the global config from `~/.config/celily/config.toml`.
    /// The config file is required.
    pub fn load_default() -> Result<Self, ConfigError> {
        validate_config_dirs()?;
        let path = config_path();
        if !path.exists() {
            return Err(ConfigError::Validation(format!(
                "{} not found; a config file with distro is required",
                path.display(),
            )));
        }
        let cfg = Self::load_file(&path)?;
        if cfg.distro.is_none() {
            return Err(ConfigError::Validation(format!(
                "{}: distro is required (e.g. distro = \"arch\")",
                path.display(),
            )));
        }
        Ok(cfg)
    }

    /// Load the global config and merge a named profile over it.
    /// Profile files live at `~/.config/celily/profiles/{name}.toml`.
    /// If the profile has an `[inherit]` entry in `profiles.toml`, the
    /// inheritance chain is walked transitively and merged from root
    /// to leaf before the profile itself is merged.
    pub fn load_with_profile(profile: &str) -> Result<Self, ConfigError> {
        let default = Self::load_default()?;
        let profiles_toml = load_profiles_toml()?;
        Self::merge_with_profile(default, profile, &profiles_toml.inherit)
    }

    /// Load the global config, then consult `~/.config/celily/profiles.toml`
    /// for a directory->profile mapping. If `cwd` matches an entry
    /// (longest-prefix), the corresponding profile is merged over the
    /// default. If the profile has an `[inherit]` entry, the inheritance
    /// chain is walked transitively and merged from root to leaf before
    /// the profile itself is merged. If no mapping exists or no entry
    /// matches, returns the default config unchanged.
    pub fn load_for_dir(cwd: &Path) -> Result<Self, ConfigError> {
        let default = Self::load_default()?;
        let cwd_canon = cwd.canonicalize().map_err(|source| ConfigError::Io {
            path: cwd.to_path_buf(),
            source,
        })?;

        let mapping = load_profiles_toml()?;
        let home = home_path()?;

        let mut best_match: Option<(PathBuf, String)> = None;
        for (raw_path, name) in &mapping.profiles {
            if !is_valid_profile_name(name) {
                warn!(%name, path = %raw_path, "skipping profiles.toml entry: invalid profile name");
                continue;
            }
            let expanded = expand_host_tilde(Path::new(raw_path), &home);
            let canon = match expanded.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    warn!(path = %raw_path, error = %e, "skipping profiles.toml entry: cannot canonicalize");
                    continue;
                },
            };
            if is_under_or_eq(&cwd_canon, &canon) {
                match &best_match {
                    Some((best, _)) if canon.components().count() > best.components().count() => {
                        best_match = Some((canon, name.clone()));
                    },
                    None => {
                        best_match = Some((canon, name.clone()));
                    },
                    _ => {},
                }
            }
        }

        match best_match {
            Some((_, profile)) => {
                info!(%profile, "matched profile");
                Self::merge_with_profile(default, &profile, &mapping.inherit)
            },
            None => Ok(default),
        }
    }

    /// Read and parse a single config TOML file.
    fn load_file(path: &Path) -> Result<Self, ConfigError> {
        validate_node_permissions(path)?;
        let content = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&content).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Load and validate a named profile, then merge it over `default`.
    ///
    /// If `inherit` maps the profile to a parent, the inheritance chain
    /// is walked transitively: each profile in the chain inherits from
    /// the next until a profile with no `[inherit]` entry is reached.
    /// Profiles are merged from root to leaf so that child values
    /// override parent values.  Self-inheritance and circular chains
    /// are rejected.
    fn merge_with_profile(
        default: Config,
        profile: &str,
        inherit: &HashMap<String, String>,
    ) -> Result<Self, ConfigError> {
        if !is_valid_profile_name(profile) {
            return Err(ConfigError::Validation(format!(
                "invalid profile name '{profile}': must not be empty, must not contain / or \\\\, \
                 must not be . or .."
            )));
        }

        // Build the inheritance chain from leaf (profile) to root.
        let mut chain: Vec<String> = Vec::new();
        let mut visited = HashSet::new();
        let mut current = profile.to_string();

        loop {
            if !visited.insert(current.clone()) {
                return Err(ConfigError::Validation(format!(
                    "circular inheritance: profile '{current}' appears more than once in \
                     the inheritance chain"
                )));
            }
            chain.push(current.clone());

            match inherit.get(&current) {
                Some(parent) => {
                    if parent == &current {
                        return Err(ConfigError::Validation(format!(
                            "profile '{current}' cannot inherit from itself"
                        )));
                    }
                    if !is_valid_profile_name(parent) {
                        return Err(ConfigError::Validation(format!(
                            "invalid parent profile name '{parent}' in [inherit] for \
                             '{current}': must not be empty, must not contain / or \\\\, \
                             must not be . or .."
                        )));
                    }
                    current = parent.clone();
                }
                None => break,
            }
        }

        // Merge from root to leaf so child values override parents.
        let mut merged = default;
        for name in chain.iter().rev() {
            let path = profile_path(name);
            if !path.exists() {
                return Err(ConfigError::Validation(format!(
                    "profile '{name}' not found at {}",
                    path.display(),
                )));
            }
            let cfg = Self::load_file(&path)?;
            merged = Self::merge(merged, cfg);
        }

        Ok(merged)
    }
}

// --- Path helpers (config-specific) ---

/// Load and parse `profiles.toml`. Returns an empty `ProfilesToml` when
/// the file does not exist.
fn load_profiles_toml() -> Result<ProfilesToml, ConfigError> {
    let path = profiles_toml_path();
    if !path.exists() {
        return Ok(ProfilesToml {
            profiles: HashMap::new(),
            inherit: HashMap::new(),
        });
    }
    validate_node_permissions(&path)?;
    let content = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;
    toml::from_str(&content).map_err(|source| ConfigError::Parse {
        path,
        source,
    })
}

/// Return the XDG config directory (`$XDG_CONFIG_HOME` or `~/.config`).
pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_default()
}

/// Path to `~/.config/celily/config.toml`.
fn config_path() -> PathBuf {
    config_dir().join("celily").join("config.toml")
}

/// Path to `~/.config/celily/profiles.toml`.
fn profiles_toml_path() -> PathBuf {
    config_dir().join("celily").join("profiles.toml")
}

/// Path to a named profile: `~/.config/celily/profiles/{name}.toml`.
fn profile_path(name: &str) -> PathBuf {
    config_dir()
        .join("celily")
        .join("profiles")
        .join(format!("{name}.toml"))
}

/// Reject profile names that are empty, contain `/` or `\\`,
/// or are `.` / `..`.
fn is_valid_profile_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

/// Return `$HOME`, or a validation error if unset.
fn home_path() -> Result<PathBuf, ConfigError> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| ConfigError::Validation("$HOME is not set".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use celily_lib::NetworkRule;

    #[test]
    fn deserialize_quota_on_allow_rule() {
        let toml = r#"
distro = "arch"

allowed_dirs = ["/tmp"]

[[network.allow]]
type = "http"
host = "api.openai.com"
quota = { max_requests = 1000, window = "1h" }

[[network.allow]]
type = "http"
host = "api.github.com"
path_prefixes = ["/repos"]
quota = { max_requests = 500, window = "30m" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let rules: Vec<NetworkRule> = config
            .network
            .allow
            .into_iter()
            .map(|r| r.into_library().unwrap())
            .collect();
        assert_eq!(rules.len(), 2);

        if let NetworkRule::Http {
            host,
            quota: Some(q),
            path_prefixes,
            ..
        } = &rules[0]
        {
            assert_eq!(host, "api.openai.com");
            assert!(path_prefixes.is_none());
            assert_eq!(q.max_requests, 1000);
            assert_eq!(q.window, std::time::Duration::from_secs(3600));
        } else {
            panic!("expected Http rule with quota");
        }

        if let NetworkRule::Http {
            host,
            quota: Some(q),
            path_prefixes: Some(prefixes),
            ..
        } = &rules[1]
        {
            assert_eq!(host, "api.github.com");
            assert_eq!(prefixes, &["/repos"]);
            assert_eq!(q.max_requests, 500);
            assert_eq!(q.window, std::time::Duration::from_secs(1800));
        } else {
            panic!("expected Http rule with quota and path_prefixes");
        }
    }

    #[test]
    fn validate_quota_rejects_zero_max_requests() {
        let config: Config = toml::from_str(
            r#"
distro = "arch"

allowed_dirs = ["/tmp"]

[[network.allow]]
type = "http"
host = "api.example.com"
quota = { max_requests = 0, window = "1h" }
"#,
        )
        .unwrap();
        let rule = config.network.allow.into_iter().next().unwrap();
        let err = rule.into_library().unwrap_err();
        assert!(err.to_string().contains("max_requests must be > 0"));
    }

    #[test]
    fn validate_quota_rejects_bad_window() {
        let config: Config = toml::from_str(
            r#"
distro = "arch"

allowed_dirs = ["/tmp"]

[[network.allow]]
type = "http"
host = "api.example.com"
quota = { max_requests = 100, window = "1x" }
"#,
        )
        .unwrap();
        let rule = config.network.allow.into_iter().next().unwrap();
        let err = rule.into_library().unwrap_err();
        assert!(err.to_string().contains("unknown window suffix"));
    }

    // -- Merge tests (moved from common.rs and run.rs) --

    fn config_with_image(img: &str) -> Config {
        Config {
            image: Some(img.into()),
            ..Default::default()
        }
    }

    #[test]
    fn image_profile_wins() {
        let merged = Config::merge(
            config_with_image("default-img"),
            config_with_image("profile-img"),
        );
        assert_eq!(merged.image.as_deref(), Some("profile-img"));
    }

    #[test]
    fn image_default_when_profile_missing() {
        let merged = Config::merge(config_with_image("default-img"), Config::default());
        assert_eq!(merged.image.as_deref(), Some("default-img"));
    }

    #[test]
    fn user_and_uid() {
        let default = Config {
            user: Some("dev".into()),
            container_uid: Some(1000),
            container_gid: Some(1000),
            ..Default::default()
        };
        let profile = Config {
            user: Some("builder".into()),
            ..Default::default()
        };
        let merged = Config::merge(default, profile);
        assert_eq!(merged.user.as_deref(), Some("builder"));
        assert_eq!(merged.container_uid, Some(1000));
        assert_eq!(merged.container_gid, Some(1000));
    }

    #[test]
    fn vm_falls_back() {
        let default = Config {
            kind: Some(InstanceKind::Vm),
            ..Default::default()
        };
        let merged = Config::merge(default, Config::default());
        assert_eq!(merged.kind, Some(InstanceKind::Vm));
    }

    #[test]
    fn security_nesting_profile_wins() {
        let default = Config {
            security_nesting: Some(false),
            ..Default::default()
        };
        let profile = Config {
            security_nesting: Some(true),
            ..Default::default()
        };
        let merged = Config::merge(default, profile);
        assert_eq!(merged.security_nesting, Some(true));
    }

    #[test]
    fn security_nesting_falls_back() {
        let default = Config {
            security_nesting: Some(true),
            ..Default::default()
        };
        let merged = Config::merge(default, Config::default());
        assert_eq!(merged.security_nesting, Some(true));
    }

    #[test]
    fn mounts_additive() {
        let mut default = Config::default();
        default.mounts = vec![Mount {
            source: "/src-a".into(),
            target: "/tgt-a".into(),
            readwrite: false,
        }];
        let mut profile = Config::default();
        profile.mounts = vec![Mount {
            source: "/src-b".into(),
            target: "/tgt-b".into(),
            readwrite: true,
        }];
        let merged = Config::merge(default, profile);
        assert_eq!(merged.mounts.len(), 2);
        assert_eq!(merged.mounts[0].source, PathBuf::from("/src-a"));
        assert_eq!(merged.mounts[1].source, PathBuf::from("/src-b"));
        assert!(merged.mounts[1].readwrite);
    }

    #[test]
    fn allowed_dirs_additive() {
        let mut default = Config::default();
        default.allowed_dirs = vec!["/a".into()];
        let mut profile = Config::default();
        profile.allowed_dirs = vec!["/b".into()];
        let merged = Config::merge(default, profile);
        assert_eq!(
            merged.allowed_dirs,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn allowed_files_additive() {
        let mut default = Config::default();
        default.allowed_files = vec!["/a".into()];
        let mut profile = Config::default();
        profile.allowed_files = vec!["/b".into()];
        let merged = Config::merge(default, profile);
        assert_eq!(
            merged.allowed_files,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn pass_env_additive() {
        let mut default = Config::default();
        default.pass_env = vec!["FOO".into()];
        let mut profile = Config::default();
        profile.pass_env = vec!["BAR".into()];
        let merged = Config::merge(default, profile);
        assert_eq!(merged.pass_env, vec!["FOO", "BAR"]);
    }

    #[test]
    fn secret_provider_profile_wins() {
        let mut default = Config::default();
        default.secret_provider = Some("rbw".into());
        let mut profile = Config::default();
        profile.secret_provider = Some("custom".into());
        let merged = Config::merge(default, profile);
        assert_eq!(merged.secret_provider.unwrap(), "custom");
    }

    #[test]
    fn secret_provider_falls_back_to_default() {
        let mut default = Config::default();
        default.secret_provider = Some("rbw".into());
        let merged = Config::merge(default, Config::default());
        assert_eq!(merged.secret_provider.unwrap(), "rbw");
    }

    #[test]
    fn env_additive() {
        let mut default = Config::default();
        default.env.insert("A".into(), "1".into());
        let mut profile = Config::default();
        profile.env.insert("B".into(), "2".into());
        profile.env.insert("A".into(), "overwritten".into());
        let merged = Config::merge(default, profile);
        assert_eq!(merged.env.len(), 2);
        assert_eq!(merged.env["A"], "overwritten");
        assert_eq!(merged.env["B"], "2");
    }

    #[test]
    fn pre_run_profile_wins() {
        let mut default = Config::default();
        default.pre_run = Some("echo default".into());
        let mut profile = Config::default();
        profile.pre_run = Some("echo profile".into());
        let merged = Config::merge(default, profile);
        assert_eq!(merged.pre_run.as_deref(), Some("echo profile"));
    }

    #[test]
    fn pre_run_default_fallback() {
        let mut default = Config::default();
        default.pre_run = Some("echo default".into());
        let merged = Config::merge(default, Config::default());
        assert_eq!(merged.pre_run.as_deref(), Some("echo default"));
    }

    #[test]
    fn pre_run_none_when_both_absent() {
        let merged = Config::merge(Config::default(), Config::default());
        assert!(merged.pre_run.is_none());
    }

    #[test]
    fn mount_project_default_is_none() {
        let merged = Config::merge(Config::default(), Config::default());
        assert!(merged.mount_project.is_none());
    }

    #[test]
    fn mount_project_profile_wins() {
        let mut profile = Config::default();
        profile.mount_project = Some(true);
        let merged = Config::merge(Config::default(), profile);
        assert_eq!(merged.mount_project, Some(true));
    }

    #[test]
    fn mount_project_falls_back_to_default() {
        let mut default = Config::default();
        default.mount_project = Some(true);
        let merged = Config::merge(default, Config::default());
        assert_eq!(merged.mount_project, Some(true));
    }
}
