use bon::Builder;

use super::rule::NetworkRule;
use crate::backend::{CreateBridgeParams, TcpAllow};

/// The fixed TCP port mitmdump listens on for HTTP proxying. Each
/// instance gets its own bridge with a unique gateway IP, so there is
/// no cross-instance port contention.
pub(crate) const PROXY_PORT: u16 = 34975;

/// The fixed port mitmdump's DNS listener uses. One above the HTTP
/// proxy port -- high enough to avoid privileged-port restrictions.
pub(crate) const DNS_PORT: u16 = PROXY_PORT + 1;

/// Network isolation parameters for an environment.
#[derive(Debug, Clone, Builder)]
pub struct NetworkParams {
    /// Allowed hosts and paths. Default-deny: anything not matched
    /// by a rule is blocked.
    #[builder(default)]
    pub allow: Vec<NetworkRule>,

    /// Enable DNS filtering through the proxy. When true, the bridge
    /// ACL restricts DNS to the gateway only, forcing all DNS traffic
    /// through mitmproxy where the allowlist is enforced.
    #[builder(default = true)]
    pub dns: bool,
}

impl NetworkParams {
    /// Build a [`CreateBridgeParams`] from these network parameters.
    ///
    /// The returned params carry the user's intent at a level the
    /// backend can translate into its own bridge configuration.
    /// The `keep` flag controls whether the bridge is preserved
    /// after the instance exits.
    pub fn to_create_bridge_params(&self, keep: bool) -> CreateBridgeParams {
        let allow_tcp: Vec<TcpAllow> = self
            .allow
            .iter()
            .filter_map(|r| match r {
                NetworkRule::Tcp { host, ports } if !ports.is_empty() => Some(TcpAllow {
                    host: *host,
                    ports: ports.clone(),
                }),
                _ => None,
            })
            .collect();

        CreateBridgeParams {
            keep,
            dns: self.dns,
            proxy_port: PROXY_PORT,
            dns_port: DNS_PORT,
            allow_tcp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(dns: bool, allow: Vec<NetworkRule>) -> NetworkParams {
        NetworkParams { allow, dns }
    }

    // ---- to_create_bridge_params ----

    #[test]
    fn to_create_bridge_params_minimal() {
        let params = make_params(true, vec![]);
        let cbp = params.to_create_bridge_params(false);
        assert!(!cbp.keep);
        assert!(cbp.dns);
        assert_eq!(cbp.proxy_port, PROXY_PORT);
        assert_eq!(cbp.dns_port, DNS_PORT);
        assert!(cbp.allow_tcp.is_empty());
    }

    #[test]
    fn to_create_bridge_params_dns_disabled() {
        let params = make_params(false, vec![]);
        let cbp = params.to_create_bridge_params(true);
        assert!(cbp.keep);
        assert!(!cbp.dns);
    }

    #[test]
    fn to_create_bridge_params_with_tcp_rules() {
        let params = make_params(
            true,
            vec![
                NetworkRule::Tcp {
                    host: "1.2.3.4".parse().unwrap(),
                    ports: vec![443, 8443],
                },
                NetworkRule::Tcp {
                    host: "5.6.7.8".parse().unwrap(),
                    ports: vec![22],
                },
            ],
        );
        let cbp = params.to_create_bridge_params(false);
        assert_eq!(cbp.allow_tcp.len(), 2);
        assert_eq!(
            cbp.allow_tcp[0].host,
            "1.2.3.4".parse::<std::net::IpAddr>().unwrap()
        );
        assert_eq!(cbp.allow_tcp[0].ports, vec![443, 8443]);
        assert_eq!(
            cbp.allow_tcp[1].host,
            "5.6.7.8".parse::<std::net::IpAddr>().unwrap()
        );
        assert_eq!(cbp.allow_tcp[1].ports, vec![22]);
    }

    #[test]
    fn to_create_bridge_params_empty_tcp_ports_skipped() {
        let params = make_params(
            true,
            vec![NetworkRule::Tcp {
                host: "1.2.3.4".parse().unwrap(),
                ports: vec![],
            }],
        );
        let cbp = params.to_create_bridge_params(false);
        assert!(cbp.allow_tcp.is_empty());
    }

    #[test]
    fn to_create_bridge_params_http_rules_ignored() {
        let params = NetworkParams {
            allow: vec![NetworkRule::Http {
                host: "api.example.com".into(),
                path_prefixes: None,
                auth: None,
                headers: std::collections::HashMap::new(),
                methods: None,
                quota: None,
            }],
            dns: true,
        };
        let cbp = params.to_create_bridge_params(false);
        assert!(cbp.allow_tcp.is_empty());
    }
}
