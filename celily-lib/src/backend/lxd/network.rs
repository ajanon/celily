use std::net::IpAddr;
use std::sync::Arc;
use std::thread;

use super::LxcBackend;
use super::acl::{self, NetworkAcl, NetworkAclRule, rule_to_cli_args};
use super::bridge::LxdBridge;
use crate::backend::{BridgeGuard, CreateBridgeParams, NetworkBackend};
use crate::command::{CommandError, CommandExt};

// ---------------------------------------------------------------------------
// Internal primitives (used by guards and the trait impl)
// ---------------------------------------------------------------------------

impl LxcBackend {
    pub(crate) fn create_network(&self, name: &str) -> Result<(), CommandError> {
        self.lxc_command()
            .args(["network", "create", name, "--type", "bridge"])
            .run()
    }

    pub(crate) fn delete_network(&self, name: &str) -> Result<(), CommandError> {
        self.lxc_command().args(["network", "delete", name]).run()
    }

    pub(crate) fn get_network_ipv4(&self, name: &str) -> Result<IpAddr, CommandError> {
        let raw = self
            .lxc_command()
            .args(["network", "get", name, "ipv4.address"])
            .run_stdout()?;
        let ip_str = raw.split('/').next().unwrap_or(&raw);
        ip_str.parse().map_err(|_| {
            CommandError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid gateway IP: {ip_str}"),
            ))
        })
    }

    pub(crate) fn set_network_dnsmasq(
        &self,
        name: &str,
        dnsmasq: &str,
    ) -> Result<(), CommandError> {
        self.lxc_command()
            .args(["network", "set", name, &format!("raw.dnsmasq={dnsmasq}")])
            .run()
    }

    pub(crate) fn attach_acl_to_network(&self, net: &str, acl: &str) -> Result<(), CommandError> {
        self.lxc_command()
            .args(["network", "set", net, &format!("security.acls={acl}")])
            .run()
    }

    pub(crate) fn set_network_egress_reject(&self, name: &str) -> Result<(), CommandError> {
        self.lxc_command()
            .args([
                "network",
                "set",
                name,
                "security.acls.default.egress.action=reject",
            ])
            .run()
    }

    pub(crate) fn create_acl(&self, name: &str) -> Result<(), CommandError> {
        self.lxc_command()
            .args(["network", "acl", "create", name])
            .run()
    }

    pub(crate) fn add_acl_rule(
        &self,
        acl: &str,
        rule: &NetworkAclRule,
    ) -> Result<(), CommandError> {
        let cli_args = rule_to_cli_args(rule);
        let mut args = vec!["network", "acl", "rule", "add", acl];
        args.extend(cli_args.iter().map(|s| s.as_str()));
        self.lxc_command().args(&args).run()
    }

    pub(crate) fn delete_acl(&self, name: &str) -> Result<(), CommandError> {
        self.lxc_command()
            .args(["network", "acl", "delete", name])
            .run()
    }
}

// ---------------------------------------------------------------------------
// LxdBridgeGuard -- wraps the two RAII guards (bridge + ACL)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct LxdBridgeGuard {
    bridge: LxdBridge,
    acl: NetworkAcl,
}

impl BridgeGuard for LxdBridgeGuard {}
// Drop order (declaration order): acl drops first, then bridge.
// Both guards are keep-aware -- if keep is true, neither is deleted.

// ---------------------------------------------------------------------------
// NetworkBackend impl
// ---------------------------------------------------------------------------

/// Build the dnsmasq configuration for DNS filtering.
///
/// When `dns` is `true`, returns a dnsmasq config that disables
/// upstream resolvers, disables caching, and forwards all queries
/// to mitmproxy's DNS listener on the gateway. When `dns` is
/// `false`, returns `None` (no DNS filtering).
fn build_dnsmasq_config(dns: bool, gateway_ip: &str, dns_port: u16) -> Option<String> {
    dns.then(|| {
        format!(
            "no-resolv
no-poll
cache-size=0
server={gateway_ip}#{dns_port}
"
        )
    })
}

impl NetworkBackend for LxcBackend {
    type Error = CommandError;

    fn create_bridge(
        &self,
        name: &str,
        params: &CreateBridgeParams,
    ) -> Result<(Box<dyn BridgeGuard>, IpAddr), Self::Error> {
        self.ensure_project()?;

        let lxd = Arc::new(self.clone());

        // RAII guard in place immediately -- any failure from here on
        // destroys the network on drop.
        let bridge = LxdBridge::create(Arc::clone(&lxd), name, params.keep)?;

        let gateway_ip = bridge.get_gateway_ip()?;
        let gateway_str = gateway_ip.to_string();

        let acl_rules = acl::build_rules_from_params(params, &gateway_str);

        // RAII guard for the ACL -- any failure from here on destroys
        // both the ACL and the bridge (in that order).
        let acl = NetworkAcl::create(Arc::clone(&lxd), name, params.keep)?;

        // Add ACL rules and set dnsmasq in parallel. Both are
        // independent -- rules mutate the ACL, dnsmasq mutates the bridge.
        let dnsmasq_config = build_dnsmasq_config(params.dns, &gateway_str, params.dns_port);

        thread::scope(|s| {
            let h1 = s.spawn(|| acl.add_rules(&acl_rules));
            let h2 = s.spawn(|| {
                if let Some(ref config) = dnsmasq_config {
                    bridge.set_raw_dnsmasq(config)
                } else {
                    Ok(())
                }
            });
            h1.join().unwrap()?;
            h2.join().unwrap()?;
            Ok::<_, CommandError>(())
        })?;

        bridge.set_default_egress_reject()?;
        bridge.attach_acl(&acl.name)?;

        let guard = LxdBridgeGuard { bridge, acl };
        Ok((Box::new(guard), gateway_ip))
    }
}
