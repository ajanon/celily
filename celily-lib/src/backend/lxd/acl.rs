//! LXD network ACL types -- rule model, RAII guard, and CLI formatting.
//!
//! These are LXD-internal. The user-facing config layer (`NetworkRule`,
//! `NetworkParams`) lives in `crate::network`.

use std::sync::Arc;

use strum::Display;
use tracing::{error, info};

use super::LxcBackend;
use crate::command::CommandError;

// ---------------------------------------------------------------------------
// ACL rule model
// ---------------------------------------------------------------------------

/// Direction for a network ACL rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Direction {
    #[allow(dead_code)]
    Ingress,
    Egress,
}

/// Action for a network ACL rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Action {
    Allow,
    #[allow(dead_code)]
    Deny,
    #[allow(dead_code)]
    Reject,
}

/// Network-layer protocol for a network ACL rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Protocol {
    Tcp,
    Udp,
    #[allow(dead_code)]
    Icmp,
    #[allow(dead_code)]
    Icmp6,
}

/// A single LXD network ACL rule.
///
/// Direction and action are required; all other fields are optional.
/// `None` fields mean "match any" at the LXD level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkAclRule {
    pub direction: Direction,
    pub action: Action,
    pub protocol: Option<Protocol>,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub source_port: Option<String>,
    pub destination_port: Option<String>,
}

// ---------------------------------------------------------------------------
// NetworkAcl -- RAII guard for a per-instance LXD network ACL
// ---------------------------------------------------------------------------

/// RAII guard for an LXD-managed network ACL.
///
/// Created via [`create`](Self::create). Rules are added with
/// [`add_rule`](Self::add_rule) or [`add_rules`](Self::add_rules).
/// The ACL is destroyed on drop unless `keep` is `true`.
pub(crate) struct NetworkAcl {
    pub(crate) name: String,
    lxd: Arc<LxcBackend>,
    keep: bool,
}

impl NetworkAcl {
    /// Create a new, empty network ACL.
    pub(crate) fn create(lxd: Arc<LxcBackend>, name: &str, keep: bool) -> Result<Self, CommandError> {
        let name = name.to_string();
        lxd.create_acl(&name)?;
        info!(acl = %name, "created network ACL");
        Ok(Self { name, lxd, keep })
    }

    /// Add a single rule to this ACL.
    pub(crate) fn add_rule(&self, rule: &NetworkAclRule) -> Result<(), CommandError> {
        self.lxd.add_acl_rule(&self.name, rule)
    }

    /// Add multiple rules to this ACL in order.
    pub(crate) fn add_rules(&self, rules: &[NetworkAclRule]) -> Result<(), CommandError> {
        for rule in rules {
            self.add_rule(rule)?;
        }
        Ok(())
    }
}

impl Drop for NetworkAcl {
    fn drop(&mut self) {
        if self.keep {
            info!(acl = %self.name, "keeping network ACL");
            return;
        }
        info!(acl = %self.name, "destroying network ACL");
        if let Err(e) = self.lxd.delete_acl(&self.name) {
            error!(acl = %self.name, error = %e, "failed to delete ACL");
        }
    }
}

// ---------------------------------------------------------------------------
// CLI formatting
// ---------------------------------------------------------------------------

/// Translate a [`NetworkAclRule`] into CLI arguments for
/// `lxc network acl rule add`.
///
/// The direction is emitted as a positional argument; all other
/// fields are emitted as `key=value` pairs. `None` fields are omitted
/// entirely (LXD treats absent filters as "any").
pub(crate) fn rule_to_cli_args(rule: &NetworkAclRule) -> Vec<String> {
    let mut args = vec![
        rule.direction.to_string(),
        format!("action={}", rule.action),
    ];
    if let Some(protocol) = rule.protocol {
        args.push(format!("protocol={protocol}"));
    }
    if let Some(ref source) = rule.source {
        args.push(format!("source={source}"));
    }
    if let Some(ref destination) = rule.destination {
        args.push(format!("destination={destination}"));
    }
    if let Some(ref source_port) = rule.source_port {
        args.push(format!("source_port={source_port}"));
    }
    if let Some(ref destination_port) = rule.destination_port {
        args.push(format!("destination_port={destination_port}"));
    }
    args
}

// ---------------------------------------------------------------------------
// Rule construction from high-level params
// ---------------------------------------------------------------------------

/// Build the list of egress ACL rules from high-level bridge parameters.
///
/// Returns:
/// - A TCP allow rule to the gateway on the proxy port (HTTP proxy).
/// - DNS UDP and TCP allow rules (restricted to the gateway when
///   `dns` is `true`, unrestricted otherwise).
/// - One TCP allow rule per entry in `allow_tcp`.
pub(crate) fn build_rules_from_params(
    params: &crate::backend::CreateBridgeParams,
    gateway_ip: &str,
) -> Vec<NetworkAclRule> {
    let mut rules = vec![
        // Allow TCP to the gateway on the proxy port (HTTP proxy).
        NetworkAclRule {
            direction: Direction::Egress,
            action: Action::Allow,
            protocol: Some(Protocol::Tcp),
            source: None,
            destination: Some(gateway_ip.to_string()),
            source_port: None,
            destination_port: Some(params.proxy_port.to_string()),
        },
        // DNS UDP -- restricted to gateway when filtering is enabled.
        NetworkAclRule {
            direction: Direction::Egress,
            action: Action::Allow,
            protocol: Some(Protocol::Udp),
            source: None,
            destination: params.dns.then(|| gateway_ip.to_string()),
            source_port: None,
            destination_port: Some("53".to_string()),
        },
        // DNS TCP -- same restriction logic.
        NetworkAclRule {
            direction: Direction::Egress,
            action: Action::Allow,
            protocol: Some(Protocol::Tcp),
            source: None,
            destination: params.dns.then(|| gateway_ip.to_string()),
            source_port: None,
            destination_port: Some("53".to_string()),
        },
    ];

    // User-specified TCP allow rules.
    for tcp in &params.allow_tcp {
        let port_list = tcp
            .ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        rules.push(NetworkAclRule {
            direction: Direction::Egress,
            action: Action::Allow,
            protocol: Some(Protocol::Tcp),
            source: None,
            destination: Some(tcp.host.to_string()),
            source_port: None,
            destination_port: Some(port_list),
        });
    }

    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_to_cli_args_minimal() {
        let rule = NetworkAclRule {
            direction: Direction::Egress,
            action: Action::Deny,
            protocol: None,
            source: None,
            destination: None,
            source_port: None,
            destination_port: None,
        };
        assert_eq!(rule_to_cli_args(&rule), vec!["egress", "action=deny"]);
    }

    #[test]
    fn rule_to_cli_args_full() {
        let rule = NetworkAclRule {
            direction: Direction::Egress,
            action: Action::Allow,
            protocol: Some(Protocol::Tcp),
            source: None,
            destination: Some("10.0.0.1".into()),
            source_port: None,
            destination_port: Some("443,8443".into()),
        };
        assert_eq!(
            rule_to_cli_args(&rule),
            vec![
                "egress",
                "action=allow",
                "protocol=tcp",
                "destination=10.0.0.1",
                "destination_port=443,8443",
            ]
        );
    }
}
