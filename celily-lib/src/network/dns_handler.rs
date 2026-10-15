//! DNS allowlist handler for the in-process proxelar proxy.
//!
//! A query is forwarded upstream only when its name is the host of an
//! HTTP allow rule: exact match, whole labels, ASCII case-insensitive.
//! Every other query gets NXDOMAIN without reaching the upstream
//! resolver. Path prefixes, methods and quotas are HTTP-only. TCP rules
//! name IP addresses, so they allow no names.
//!
//! Without this filter, a process in the instance could still send data
//! out through the names it looks up, to an authoritative server it
//! controls, even with every other destination blocked.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use proxyapi::{DnsDecision, DnsHandler, DnsName};
use tracing::warn;

use super::rule::NetworkRule;

/// DNS handler enforcing celily's allowlist.
///
/// `Clone` is cheap: the proxy clones the handler for every query.
#[derive(Clone)]
pub(crate) struct CelilyDnsHandler {
    /// Hosts of the HTTP allow rules.
    allowed: Arc<HashSet<DnsName>>,
}

impl CelilyDnsHandler {
    pub(crate) fn new(rules: &[NetworkRule]) -> Self {
        let allowed = rules
            .iter()
            .filter_map(|rule| {
                let NetworkRule::Http { host, .. } = rule else {
                    return None;
                };
                host.parse()
                    .inspect_err(|e| warn!(%host, error = %e, "not a DNS name, never resolvable"))
                    .ok()
            })
            .collect();
        Self {
            allowed: Arc::new(allowed),
        }
    }
}

#[async_trait]
impl DnsHandler for CelilyDnsHandler {
    async fn handle_query(&self, name: &DnsName, _query_type: u16) -> DnsDecision {
        if self.allowed.contains(name) {
            DnsDecision::Forward
        } else {
            DnsDecision::NxDomain
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const A: u16 = 1;
    const AAAA: u16 = 28;

    fn http_rule(host: &str) -> NetworkRule {
        NetworkRule::Http {
            host: host.into(),
            path_prefixes: None,
            auth: None,
            headers: HashMap::new(),
            methods: None,
            quota: None,
        }
    }

    async fn decide(handler: &CelilyDnsHandler, name: &str) -> DnsDecision {
        handler.handle_query(&name.parse().unwrap(), A).await
    }

    #[tokio::test]
    async fn rule_hosts_are_forwarded() {
        let h = CelilyDnsHandler::new(&[http_rule("api.example.com"), http_rule("crates.io")]);
        assert_eq!(decide(&h, "api.example.com").await, DnsDecision::Forward);
        assert_eq!(decide(&h, "crates.io").await, DnsDecision::Forward);
        let name = "crates.io".parse().unwrap();
        assert_eq!(h.handle_query(&name, AAAA).await, DnsDecision::Forward);
    }

    #[tokio::test]
    async fn match_ignores_case_and_the_root_dot() {
        let h = CelilyDnsHandler::new(&[http_rule("API.Example.com")]);
        assert_eq!(decide(&h, "api.example.COM.").await, DnsDecision::Forward);
    }

    #[tokio::test]
    async fn other_names_get_nxdomain() {
        let h = CelilyDnsHandler::new(&[http_rule("api.example.com")]);
        assert_eq!(decide(&h, "evil.com").await, DnsDecision::NxDomain);
        assert_eq!(decide(&h, "example.com").await, DnsDecision::NxDomain);
        assert_eq!(decide(&h, "x.api.example.com").await, DnsDecision::NxDomain);
    }

    #[tokio::test]
    async fn label_boundaries_are_compared() {
        let h = CelilyDnsHandler::new(&[http_rule("api.example.com")]);
        // Two labels, the first one containing a dot: not api.example.com.
        let name = DnsName::from_labels(["api.example", "com"]).unwrap();
        assert_eq!(h.handle_query(&name, A).await, DnsDecision::NxDomain);
    }

    #[tokio::test]
    async fn tcp_rules_allow_no_names() {
        let tcp = NetworkRule::Tcp {
            host: "10.0.0.5".parse().unwrap(),
            ports: vec![22],
        };
        let h = CelilyDnsHandler::new(&[tcp]);
        assert_eq!(decide(&h, "10.0.0.5").await, DnsDecision::NxDomain);
    }
}
