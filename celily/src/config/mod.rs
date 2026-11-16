pub mod backend;
pub mod limits;
pub mod merge_strategy;
pub mod network;
pub mod proxy;
pub mod validation;
pub mod volume;
pub mod worktree;

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[cfg(test)]
use celily_lib::AccessMode;
use celily_lib::{DistroKind, InstanceKind, Mount};
use merge::Merge;
use serde::Deserialize;
use tracing::{info, warn};

pub use self::backend::{BackendConfig, BackendKind};
pub use self::limits::Limits;
pub use self::network::NetworkConfig;
use self::validation::{validate_config_dirs, validate_node_permissions};
pub use self::volume::Volume;
pub use self::worktree::WorktreeConfig;
use crate::config::proxy::ProxyDevice;
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

#[derive(Debug, Default, Deserialize, Merge)]
#[serde(default, deny_unknown_fields)]
#[merge(strategy = merge_strategy::overwrite_some)]
pub struct Config {
    // -- Backend selection --
    #[merge(strategy = Merge::merge)]
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
    #[merge(strategy = Merge::merge)]
    pub limits: Limits,
    #[merge(strategy = ::merge::vec::append)]
    pub mounts: Vec<Mount>,
    /// Absolute directory paths that may be bind-mounted. Must be an exact
    /// match (not a subtree).
    #[merge(strategy = ::merge::vec::append)]
    pub allowed_dirs: Vec<PathBuf>,
    /// Absolute file paths that may be bind-mounted. Must be an exact match.
    #[merge(strategy = ::merge::vec::append)]
    pub allowed_files: Vec<PathBuf>,
    /// Static environment variables set for every `lxc exec` inside the
    /// container. CLI `--env` overrides take precedence.
    #[merge(strategy = ::merge::hashmap::overwrite)]
    pub env: HashMap<String, String>,
    /// Target path inside the container for the project bind-mount. Tilde is
    /// expanded to the container home directory. Default: `~/project`.
    pub project_target: Option<String>,
    /// Names of host environment variables to forward into the container.
    /// Overrides static `env` entries for the same key.
    #[merge(strategy = ::merge::vec::append)]
    pub pass_env: Vec<String>,
    /// Name of the active secret provider. Available providers are listed
    /// in `celily-config`(5). When absent, secrets are not resolved;
    /// attempting to use `auth.secret` in a network allow rule without a
    /// configured provider is an error.
    pub secret_provider: Option<celily_lib::Providers>,
    /// Network isolation configuration. When enabled, the instance gets a
    /// dedicated bridge with an egress ACL and a per-instance MITM proxy.
    #[merge(strategy = Merge::merge)]
    pub network: NetworkConfig,
    /// Mount the project directory read-write, `.git` included. Ignored in
    /// worktree mode.
    pub project_readwrite: Option<bool>,
    /// Worktree configuration. Worktree mode is activated by passing
    /// `--worktree NAME` on the CLI; this section configures its
    /// behaviour. See `celily`(1) for details.
    #[merge(strategy = Merge::merge)]
    pub worktree: WorktreeConfig,
    /// Mount the project directory (current working directory) into the
    /// container. When `None`, the project directory is not mounted unless
    /// worktree mode is active (which always mounts it). Set to `true` to
    /// opt in, `false` to explicitly disable.
    pub mount_project: Option<bool>,
    /// Inline scripts run as the container user before the main command
    /// (or `bash --login`). Each fragment sets at most one; merging
    /// appends them, so they run in the order the profile lists its
    /// fragments. The first failure aborts the run.
    /// Shebang-aware: each is written to a temp file and executed, so
    /// `#!/usr/bin/env python3` works. The same environment variables as
    /// the main command are set, plus `CELILY_USER`, `CELILY_UID`,
    /// `CELILY_GID`, `CELILY_HOME`. Empty string or whitespace-only is
    /// treated as not set.
    #[serde(deserialize_with = "deserialize_pre_run")]
    #[merge(strategy = ::merge::vec::append)]
    pub pre_run: Vec<String>,
    /// Proxy devices that expose host Unix sockets inside the instance.
    /// Each entry corresponds to an LXD/Incus proxy device.
    #[merge(strategy = ::merge::vec::append)]
    pub proxy: Vec<ProxyDevice>,
    /// Existing custom storage volumes of `[backend] pool` to attach.
    #[merge(strategy = ::merge::vec::append)]
    pub volumes: Vec<Volume>,
}

/// Deserialize one file's `pre_run` script into the list that profile
/// merging appends to. Trimmed so a shebang lands on the first byte;
/// empty scripts are dropped here and never run.
fn deserialize_pre_run<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let script = String::deserialize(d)?;
    let trimmed = script.trim();
    if trimmed.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(vec![trimmed.to_owned()])
    }
}

/// Parsed representation of `profiles.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilesToml {
    /// Directory -> profile name, longest prefix wins.
    #[serde(default)]
    directories: HashMap<String, String>,
    /// Profile name -> fragment names, merged in list order.
    profiles: HashMap<String, Vec<String>>,
}

impl ProfilesToml {
    /// Read, parse and validate `profiles.toml` in `dir`. The file is
    /// required: every config comes from a profile.
    fn load(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join("profiles.toml");
        if !path.exists() {
            return Err(ConfigError::Validation(format!(
                "{} not found; it defines the profiles celily runs with",
                path.display(),
            )));
        }
        validate_node_permissions(&path)?;
        let content = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let parsed: Self = toml::from_str(&content).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        parsed
            .validate()
            .map_err(|e| ConfigError::Validation(format!("{}: {e}", path.display())))?;
        Ok(parsed)
    }

    /// Check the whole file, not only the profile about to be used: a
    /// mistake shows up on the next run, not on the day a directory
    /// happens to match. Fragment files are checked when loaded.
    fn validate(&self) -> Result<(), String> {
        for (profile, fragments) in &self.profiles {
            if fragments.is_empty() {
                return Err(format!("profile '{profile}' lists no fragments"));
            }
            let mut seen = HashSet::new();
            for name in fragments {
                if !is_valid_fragment_name(name) {
                    return Err(format!(
                        "profile '{profile}': invalid fragment name '{name}': must not be empty, \
                         must not contain / or \\, must not be . or .."
                    ));
                }
                if !seen.insert(name) {
                    return Err(format!("profile '{profile}' lists fragment '{name}' twice"));
                }
            }
        }
        for (dir, profile) in &self.directories {
            if !self.profiles.contains_key(profile) {
                return Err(format!(
                    "[directories] maps '{dir}' to profile '{profile}', which [profiles] does not \
                     define"
                ));
            }
        }
        Ok(())
    }

    /// The profile of the longest `[directories]` entry containing
    /// `cwd_canon`. Entries that cannot be canonicalized (e.g. on an
    /// unmounted drive) are skipped. Two entries resolving to the same
    /// directory (`~/w`, `/home/me/w/`) must map to the same profile,
    /// whether they match `cwd_canon` or not: the winner of such a tie
    /// would depend on the hash order, which changes on every run.
    fn profile_for_dir(&self, cwd_canon: &Path, home: &Path) -> Result<Option<&str>, String> {
        let mut resolved: HashMap<PathBuf, (&str, &str)> = HashMap::new();
        for (raw_path, name) in &self.directories {
            let expanded = expand_host_tilde(Path::new(raw_path), home);
            let canon = match expanded.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    warn!(path = %raw_path, error = %e, "skipping [directories] entry: cannot canonicalize");
                    continue;
                },
            };
            match resolved.entry(canon) {
                Entry::Occupied(e) if e.get().1 != name => {
                    let (other_path, other_name) = e.get();
                    return Err(format!(
                        "[directories] entries '{other_path}' and '{raw_path}' both resolve to \
                         {}, but map to different profiles ('{other_name}' and '{name}')",
                        e.key().display(),
                    ));
                },
                Entry::Occupied(_) => {},
                Entry::Vacant(e) => {
                    e.insert((raw_path, name));
                },
            }
        }
        // Matching entries are all ancestors of `cwd_canon`, distinct
        // after the check above: their lengths differ, no tie is left.
        Ok(resolved
            .into_iter()
            .filter(|(canon, _)| is_under_or_eq(cwd_canon, canon))
            .max_by_key(|(canon, _)| canon.components().count())
            .map(|(_, (_, name))| name))
    }
}

impl Config {
    /// Load the named profile from `~/.config/celily/profiles.toml`.
    pub fn load_with_profile(profile: &str) -> Result<Self, ConfigError> {
        let dir = celily_config_dir();
        validate_config_dirs(&dir)?;
        let profiles = ProfilesToml::load(&dir)?;
        Self::load_profile(&dir, &profiles, profile)
    }

    /// Load the profile that `~/.config/celily/profiles.toml` maps to
    /// `cwd` (longest `[directories]` prefix). No match is an error:
    /// there is no implicit default, `"/"` maps every directory.
    pub fn load_for_dir(cwd: &Path) -> Result<Self, ConfigError> {
        let dir = celily_config_dir();
        validate_config_dirs(&dir)?;
        let profiles = ProfilesToml::load(&dir)?;
        let cwd_canon = cwd.canonicalize().map_err(|source| ConfigError::Io {
            path: cwd.to_path_buf(),
            source,
        })?;
        let home = home_path()?;
        let profile = profiles.profile_for_dir(&cwd_canon, &home).map_err(|e| {
            ConfigError::Validation(format!("{}: {e}", dir.join("profiles.toml").display()))
        })?;
        let Some(profile) = profile else {
            return Err(ConfigError::Validation(format!(
                "no [directories] entry in {} matches {}; add one (\"/\" matches every directory) \
                 or pass --profile",
                dir.join("profiles.toml").display(),
                cwd_canon.display(),
            )));
        };
        info!(%profile, "matched profile");
        Self::load_profile(&dir, &profiles, profile)
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

    /// Merge the fragments of `profile` (`dir/fragments/NAME.toml`) in
    /// list order: later fragments override earlier ones. The result
    /// must set `distro`.
    fn load_profile(
        dir: &Path,
        profiles: &ProfilesToml,
        profile: &str,
    ) -> Result<Self, ConfigError> {
        let fragments = profiles.profiles.get(profile).ok_or_else(|| {
            ConfigError::Validation(format!(
                "profile '{profile}' is not defined in [profiles] of {}",
                dir.join("profiles.toml").display(),
            ))
        })?;
        info!(%profile, fragments = %fragments.join(" "), "loading profile");

        let mut merged = Config::default();
        for name in fragments {
            let path = dir.join("fragments").join(format!("{name}.toml"));
            if !path.exists() {
                return Err(ConfigError::Validation(format!(
                    "fragment '{name}' of profile '{profile}' not found at {}",
                    path.display(),
                )));
            }
            merged.merge(Self::load_file(&path)?);
        }

        if merged.distro.is_none() {
            return Err(ConfigError::Validation(format!(
                "profile '{profile}': distro is required; set it in one of its fragments (e.g. \
                 distro = \"arch\")"
            )));
        }
        Ok(merged)
    }
}

// --- Path helpers (config-specific) ---

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

/// `~/.config/celily`: `profiles.toml` and `fragments/`.
fn celily_config_dir() -> PathBuf {
    config_dir().join("celily")
}

/// Reject fragment names that are empty, contain `/` or `\\`,
/// or are `.` / `..`: they become file names.
fn is_valid_fragment_name(name: &str) -> bool {
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
    use celily_lib::NetworkRule;

    use super::*;

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
    fn unknown_fields_are_rejected() {
        let base = "distro = \"arch\"\nallowed_dirs = [\"/tmp\"]\n";
        for typo in [
            "vms = true",
            "[limits]\ncpus = 2",
            "[backend]\npoool = \"p\"",
            "[worktree]\nusername = \"me\"",
            "[network]\nallowed = []",
            "[[network.allow]]\ntype = \"http\"\nhost = \"a\"\nmethod = [\"POST\"]",
            "[[network.allow]]\ntype = \"tcp\"\nhost = \"10.0.0.1\"\nports = [1]\nport = 2",
            "[[network.allow]]\ntype = \"http\"\nhost = \"a\"\nquota = { max_requests = 1, window \
             = \"1h\", burst = 2 }",
            "[[network.allow]]\ntype = \"http\"\nhost = \"a\"\nauth = { secret = \"s\", header = \
             \"h\", prefx = \"p\" }",
            "[[mounts]]\nsource = \"/a\"\ntarget = \"/b\"\nreadonly = true",
            "[[proxy]]\nconnect = \"unix:/a\"\nlisten = \"unix:/b\"\nbind = \"host\"",
            "[[volumes]]\nname = \"v\"\ntarget = \"/b\"\npool = \"p\"",
        ] {
            let err = toml::from_str::<Config>(&format!("{base}{typo}\n")).unwrap_err();
            assert!(err.to_string().contains("unknown field"), "{typo}: {err}");
        }
        let err = toml::from_str::<ProfilesToml>("[profiles]\n[inherits]\n").err();
        assert!(err.is_some_and(|e| e.to_string().contains("unknown field")));
    }

    // -- profiles.toml --

    fn profiles_toml(s: &str) -> ProfilesToml {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn profiles_toml_validation() {
        let ok = "[directories]\n\"/\" = \"p\"\n[profiles]\np = [\"base\", \"p\"]\n";
        assert_eq!(profiles_toml(ok).validate(), Ok(()));

        for (toml, expected) in [
            ("[profiles]\np = []", "lists no fragments"),
            (
                "[profiles]\np = [\"a\", \"b\", \"a\"]",
                "lists fragment 'a' twice",
            ),
            ("[profiles]\np = [\"../a\"]", "invalid fragment name '../a'"),
            ("[profiles]\np = [\"..\"]", "invalid fragment name '..'"),
            (
                "[directories]\n\"/\" = \"q\"\n[profiles]\np = [\"a\"]",
                "'q', which [profiles]",
            ),
        ] {
            let err = profiles_toml(toml).validate().unwrap_err();
            assert!(err.contains(expected), "{toml}: {err}");
        }

        // The old single-parent form no longer parses.
        assert!(toml::from_str::<ProfilesToml>("[profiles]\np = \"a\"").is_err());
        assert!(toml::from_str::<ProfilesToml>("[profiles]\n[inherit]\np = \"a\"").is_err());
    }

    /// Longest prefix wins, `"/"` catches everything else, entries that
    /// do not exist are skipped.
    #[test]
    fn profile_for_dir_longest_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        let profiles = profiles_toml(&format!(
            "[directories]\n\"/\" = \"all\"\n\"{a}\" = \"a\"\n\"{gone}\" = \
             \"gone\"\n[profiles]\nall = [\"x\"]\na = [\"x\"]\ngone = [\"x\"]\n",
            a = root.join("a").display(),
            gone = root.join("gone").display(),
        ));
        let home = Path::new("/nonexistent-home");
        assert_eq!(
            profiles.profile_for_dir(&root.join("a/b"), home),
            Ok(Some("a"))
        );
        assert_eq!(
            profiles.profile_for_dir(&root.join("a"), home),
            Ok(Some("a"))
        );
        assert_eq!(profiles.profile_for_dir(&root, home), Ok(Some("all")));

        let no_catch_all = profiles_toml(&format!(
            "[directories]\n\"{a}\" = \"a\"\n[profiles]\na = [\"x\"]\n",
            a = root.join("a").display(),
        ));
        assert_eq!(no_catch_all.profile_for_dir(&root, home), Ok(None));
    }

    /// Two entries resolving to one directory: fine with one profile, an
    /// error with two, even when `cwd` is elsewhere.
    #[test]
    fn profile_for_dir_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        let home = root.as_path();
        let with = |first: &str, second: &str| {
            profiles_toml(&format!(
                "[directories]\n\"/\" = \"all\"\n\"~/a\" = \"{first}\"\n\"{a}/\" = \
                 \"{second}\"\n[profiles]\nall = [\"x\"]\na = [\"x\"]\nb = [\"x\"]\n",
                a = root.join("a").display(),
            ))
        };

        assert_eq!(
            with("a", "a").profile_for_dir(&root.join("a"), home),
            Ok(Some("a"))
        );
        for cwd in [root.join("a"), root.join("elsewhere")] {
            let err = with("a", "b").profile_for_dir(&cwd, home).unwrap_err();
            assert!(
                err.contains("different profiles"),
                "{}: {err}",
                cwd.display()
            );
        }
    }

    /// Fragments merge in list order, from `dir/fragments/NAME.toml`.
    #[test]
    fn load_profile_merges_fragments_in_order() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir(dir.join("fragments")).unwrap();
        for (name, content) in [
            (
                "base",
                "distro = \"arch\"\nimage = \"base\"\npre_run = \"base\"",
            ),
            ("x", "image = \"x\"\npre_run = \"x\""),
        ] {
            let path = dir.join("fragments").join(format!("{name}.toml"));
            std::fs::write(&path, content).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let profiles = profiles_toml(
            "[profiles]\nbx = [\"base\", \"x\"]\nxb = [\"x\", \"base\"]\nx = [\"x\"]\nmissing = \
             [\"base\", \"nope\"]\n",
        );

        let cfg = Config::load_profile(dir, &profiles, "bx").unwrap();
        assert_eq!(cfg.image.as_deref(), Some("x"));
        assert_eq!(cfg.pre_run, ["base", "x"]);

        let cfg = Config::load_profile(dir, &profiles, "xb").unwrap();
        assert_eq!(cfg.image.as_deref(), Some("base"));
        assert_eq!(cfg.pre_run, ["x", "base"]);

        for (profile, expected) in [
            ("x", "distro is required"),
            ("missing", "fragment 'nope' of profile 'missing' not found"),
            ("undefined", "profile 'undefined' is not defined"),
        ] {
            let err = Config::load_profile(dir, &profiles, profile).unwrap_err();
            assert!(err.to_string().contains(expected), "{profile}: {err}");
        }
    }

    // -- Merge tests (derive + strategies) --

    /// Profile scalar wins over default.
    #[test]
    fn scalar_profile_wins() {
        let mut default = Config::default();
        default.image = Some("default".into());
        let mut profile = Config::default();
        profile.image = Some("profile".into());

        default.merge(profile);
        assert_eq!(default.image.as_deref(), Some("profile"));
    }

    /// Default scalar preserved when profile is None.
    #[test]
    fn scalar_default_fallback() {
        let mut default = Config::default();
        default.image = Some("default".into());
        let profile = Config::default();

        default.merge(profile);
        assert_eq!(default.image.as_deref(), Some("default"));
    }

    /// Vec fields: default first, profile appended.
    #[test]
    fn vec_concatenation() {
        let mut default = Config::default();
        default.mounts = vec![Mount {
            source: "/src-a".into(),
            target: "/tgt-a".into(),
            access: AccessMode::ReadOnly,
        }];
        let mut profile = Config::default();
        profile.mounts = vec![Mount {
            source: "/src-b".into(),
            target: "/tgt-b".into(),
            access: AccessMode::ReadWrite,
        }];

        default.merge(profile);
        assert_eq!(default.mounts.len(), 2);
        assert_eq!(default.mounts[0].source, PathBuf::from("/src-a"));
        assert_eq!(default.mounts[1].source, PathBuf::from("/src-b"));
        assert_eq!(default.mounts[1].access, AccessMode::ReadWrite);
    }

    /// `pre_run`: one script per file, trimmed, empty means none.
    #[test]
    fn deserialize_pre_run_script() {
        let config: Config = toml::from_str("pre_run = '''\n#!/bin/sh\ntrue\n'''").unwrap();
        assert_eq!(config.pre_run, vec!["#!/bin/sh\ntrue".to_owned()]);

        let config: Config = toml::from_str("pre_run = \" \\n \"").unwrap();
        assert_eq!(config.pre_run, Vec::<String>::new());

        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.pre_run, Vec::<String>::new());
    }

    /// `pre_run` scripts accumulate in merge order instead of replacing.
    #[test]
    fn pre_run_appends() {
        let mut default: Config = toml::from_str("pre_run = 'default'").unwrap();
        let parent: Config = toml::from_str("pre_run = 'parent'").unwrap();
        let unset: Config = toml::from_str("").unwrap();
        let profile: Config = toml::from_str("pre_run = 'profile'").unwrap();

        default.merge(parent);
        default.merge(unset);
        default.merge(profile);
        assert_eq!(default.pre_run, ["default", "parent", "profile"]);
    }

    /// HashMap fields: extended, profile keys win on conflict.
    #[test]
    fn hashmap_extend_override() {
        let mut default = Config::default();
        default.env.insert("A".into(), "default".into());
        let mut profile = Config::default();
        profile.env.insert("A".into(), "profile".into());
        profile.env.insert("B".into(), "new".into());

        default.merge(profile);
        assert_eq!(default.env.len(), 2);
        assert_eq!(default.env["A"], "profile");
        assert_eq!(default.env["B"], "new");
    }

    /// Nested structs: profile fields override default inside them.
    #[test]
    fn nested_struct_merge() {
        let mut default = Config::default();
        default.user = Some("dev".into());
        default.container_uid = Some(1000);
        default.container_gid = Some(1000);

        let mut profile = Config::default();
        profile.user = Some("builder".into());

        default.merge(profile);
        assert_eq!(default.user.as_deref(), Some("builder"));
        assert_eq!(default.container_uid, Some(1000));
        assert_eq!(default.container_gid, Some(1000));
    }

    /// Option<bool> defaulting to true: profile `false` wins.
    #[test]
    fn option_bool_false_wins() {
        let mut default = Config::default();
        default.secure_boot = Some(true);
        let mut profile = Config::default();
        profile.secure_boot = Some(false);

        default.merge(profile);
        assert_eq!(default.secure_boot, Some(false));
    }

    /// Nested BackendConfig: profile fields inside it win.
    #[test]
    fn nested_backend_merge() {
        let mut default = Config::default();
        default.backend.project = Some("default-pool".into());
        default.backend.pool = Some("default".into());

        let mut profile = Config::default();
        profile.backend.project = Some("profile-project".into());

        default.merge(profile);
        assert_eq!(default.backend.project.as_deref(), Some("profile-project"));
        // pool was only in default, preserved
        assert_eq!(default.backend.pool.as_deref(), Some("default"));
    }
}
