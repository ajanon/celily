use std::collections::HashMap;
use std::time::Duration;

use celily_lib::{AuthConfig, NetworkRule, QuotaConfig};
use merge::Merge;
use serde::Deserialize;

use super::ConfigError;

/// Network isolation configuration (config-file representation).
///
/// Holds raw deserialized types. Call [`ConfigNetworkRule::into_library`]
/// to validate and convert to the library's [`NetworkRule`].
#[derive(Debug, Default, Clone, Deserialize, Merge)]
#[serde(default)]
#[merge(strategy = super::merge_strategy::overwrite_some)]
pub struct NetworkConfig {
    /// Allowed hosts/paths. Default-deny: anything not matched is
    /// blocked with HTTP 403.
    #[merge(strategy = ::merge::vec::append)]
    pub allow: Vec<ConfigNetworkRule>,

    /// Enable DNS filtering via the mitmproxy DNS listener.
    #[serde(default = "default_dns")]
    pub dns: Option<bool>,
}

fn default_dns() -> Option<bool> {
    Some(true)
}

/// Config-file representation of a single network isolation rule.
///
/// Mirrors [`NetworkRule`] but uses [`ConfigQuotaConfig`] for the
/// quota field (raw string window, validated on conversion).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ConfigNetworkRule {
    #[serde(rename = "http")]
    Http {
        host: String,
        #[serde(default)]
        path_prefixes: Option<Vec<String>>,
        #[serde(default)]
        auth: Option<AuthConfig>,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        methods: Option<Vec<String>>,
        #[serde(default)]
        quota: Option<ConfigQuotaConfig>,
    },

    #[serde(rename = "tcp")]
    Tcp {
        host: std::net::IpAddr,
        ports: Vec<u16>,
    },
}

impl ConfigNetworkRule {
    /// Validate and convert to a library [`NetworkRule`].
    pub fn into_library(self) -> Result<NetworkRule, ConfigError> {
        match self {
            Self::Http {
                host,
                path_prefixes,
                auth,
                headers,
                methods,
                quota,
            } => {
                let quota = quota.map(ConfigQuotaConfig::into_library).transpose()?;
                Ok(NetworkRule::Http {
                    host,
                    path_prefixes,
                    auth,
                    headers,
                    methods,
                    quota,
                })
            },
            Self::Tcp { host, ports } => Ok(NetworkRule::Tcp { host, ports }),
        }
    }
}

/// Config-file representation of [`QuotaConfig`].
///
/// The `window` field is a human-readable string like `"1h"`;
/// [`into_library`](Self::into_library) parses it into a [`Duration`].
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigQuotaConfig {
    pub max_requests: u64,
    pub window: String,
}

impl ConfigQuotaConfig {
    /// Validate and convert to a library [`QuotaConfig`].
    pub fn into_library(self) -> Result<QuotaConfig, ConfigError> {
        if self.max_requests == 0 {
            return Err(ConfigError::Validation("max_requests must be > 0".into()));
        }
        let window = parse_window_duration(&self.window).map_err(ConfigError::Validation)?;
        Ok(QuotaConfig {
            max_requests: self.max_requests,
            window,
        })
    }
}

/// Parse a human-readable duration string like `"1h"`, `"30m"`, `"86400s"`,
/// or `"7d"` into a [`Duration`].
///
/// Supported suffixes: `s` (seconds), `m` (minutes), `h` (hours),
/// `d` (days).
fn parse_window_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty window duration".into());
    }
    let (num_str, suffix) = s.split_at(s.len() - 1);
    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number in window '{s}': expected <number><suffix>"))?;
    let multiplier = match suffix {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => {
            return Err(format!(
                "unknown window suffix '{suffix}' in '{s}'; expected s, m, h, or d"
            ));
        },
    };
    Ok(Duration::from_secs(num * multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_window_duration ---

    #[test]
    fn parse_suffixes() {
        assert_eq!(
            parse_window_duration("1h").unwrap(),
            Duration::from_secs(3600)
        );
        assert_eq!(
            parse_window_duration("30m").unwrap(),
            Duration::from_secs(1800)
        );
        assert_eq!(
            parse_window_duration("86400s").unwrap(),
            Duration::from_secs(86400)
        );
        assert_eq!(
            parse_window_duration("7d").unwrap(),
            Duration::from_secs(604800)
        );
    }

    #[test]
    fn parse_rejects_invalid() {
        assert!(parse_window_duration("1x").is_err());
        assert!(parse_window_duration("").is_err());
        assert!(parse_window_duration("abc").is_err());
    }

    // --- ConfigNetworkRule deserialization ---

    #[test]
    fn deserialize_quota_on_allow_rule() {
        let toml = r#"
[[allow]]
type = "http"
host = "api.example.com"
quota = { max_requests = 1000, window = "1h" }
"#;
        let nc: NetworkConfig = toml::from_str(toml).unwrap();
        let rule = nc.allow.into_iter().next().unwrap();
        let lib_rule = rule.into_library().unwrap();
        if let NetworkRule::Http {
            host,
            quota: Some(q),
            ..
        } = lib_rule
        {
            assert_eq!(host, "api.example.com");
            assert_eq!(q.max_requests, 1000);
            assert_eq!(q.window, Duration::from_secs(3600));
        } else {
            panic!("expected Http rule with quota");
        }
    }

    #[test]
    fn deserialize_rejects_zero_max_requests() {
        let toml = r#"
[[allow]]
type = "http"
host = "api.example.com"
quota = { max_requests = 0, window = "1h" }
"#;
        let nc: NetworkConfig = toml::from_str(toml).unwrap();
        let rule = nc.allow.into_iter().next().unwrap();
        let err = rule.into_library().unwrap_err();
        assert!(err.to_string().contains("max_requests must be > 0"));
    }

    #[test]
    fn deserialize_tcp_valid_ip() {
        let toml = r#"
[[allow]]
type = "tcp"
host = "10.0.0.5"
ports = [22, 8443]
"#;
        let nc: NetworkConfig = toml::from_str(toml).unwrap();
        let rule = nc.allow.into_iter().next().unwrap();
        let lib_rule = rule.into_library().unwrap();
        if let NetworkRule::Tcp { host, ports } = lib_rule {
            assert_eq!(host, "10.0.0.5".parse::<std::net::IpAddr>().unwrap());
            assert_eq!(ports, vec![22, 8443]);
        } else {
            panic!("expected Tcp rule");
        }
    }

    #[test]
    fn deserialize_tcp_rejects_hostname() {
        let toml = r#"
[[allow]]
type = "tcp"
host = "api.example.com"
ports = [443]
"#;
        let err = toml::from_str::<NetworkConfig>(toml).unwrap_err();
        assert!(err.to_string().contains("IP address") || err.to_string().contains("invalid IP"));
    }

    #[test]
    fn deserialize_tcp_valid_ipv6() {
        let toml = r#"
[[allow]]
type = "tcp"
host = "::1"
ports = [22]
"#;
        let nc: NetworkConfig = toml::from_str(toml).unwrap();
        let rule = nc.allow.into_iter().next().unwrap();
        let lib_rule = rule.into_library().unwrap();
        if let NetworkRule::Tcp { host, .. } = lib_rule {
            assert_eq!(host, "::1".parse::<std::net::IpAddr>().unwrap());
        } else {
            panic!("expected Tcp rule");
        }
    }

    #[test]
    fn deserialize_rejects_bad_window() {
        let toml = r#"
[[allow]]
type = "http"
host = "api.example.com"
quota = { max_requests = 100, window = "1x" }
"#;
        let nc: NetworkConfig = toml::from_str(toml).unwrap();
        let rule = nc.allow.into_iter().next().unwrap();
        let err = rule.into_library().unwrap_err();
        assert!(err.to_string().contains("unknown window suffix"));
    }
}
