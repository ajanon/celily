//! RAII guard for a per-instance LXD-managed network bridge.
//!
//! The bridge is created with LXD defaults (auto-assigned subnet, DHCP, NAT).
//! The gateway IP is queried lazily on first access via
//! [`get_gateway_ip`](LxdBridge::get_gateway_ip).
//!
//! Callers compose the bridge with ACLs and dnsmasq configuration
//! externally: [`attach_acl`](LxdBridge::attach_acl),
//! [`set_default_egress_reject`](LxdBridge::set_default_egress_reject), and
//! [`set_raw_dnsmasq`](LxdBridge::set_raw_dnsmasq) are the building blocks.

use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

use tracing::{error, info};

use super::LxcBackend;
use crate::command::CommandError;

pub(crate) struct LxdBridge {
    pub name: String,
    lxd: Arc<LxcBackend>,
    gateway_ip: OnceLock<IpAddr>,
    keep: bool,
}

impl LxdBridge {
    /// Create the bridge with LXD defaults (auto-assigned subnet, DHCP, NAT).
    ///
    /// Returns immediately after `lxc network create` so that the RAII guard
    /// is in place. The gateway IP is queried lazily on first call to
    /// [`get_gateway_ip`](Self::get_gateway_ip) -- the split guarantees that
    /// `Drop` still runs and destroys the bridge if any later step fails.
    pub(crate) fn create(
        lxd: Arc<LxcBackend>,
        name: &str,
        keep: bool,
    ) -> Result<Self, CommandError> {
        let name = name.to_string();
        lxd.create_network(&name)?;
        info!(bridge = %name, "created isolation bridge");
        Ok(Self {
            name,
            lxd,
            gateway_ip: OnceLock::new(),
            keep,
        })
    }

    /// Query the gateway IP from LXD, caching the result.
    pub(crate) fn get_gateway_ip(&self) -> Result<IpAddr, CommandError> {
        self.gateway_ip
            .get_or_try_init(|| {
                let ip = self.lxd.get_network_ipv4(&self.name)?;
                info!(bridge = %self.name, gateway = %ip, "queried bridge gateway IP");
                Ok(ip)
            })
            .map(|ip| *ip)
    }

    /// Set the `raw.dnsmasq` config key on this bridge.
    pub(crate) fn set_raw_dnsmasq(&self, dnsmasq: &str) -> Result<(), CommandError> {
        info!(bridge = %self.name, "setting raw.dnsmasq on bridge");
        self.lxd.set_network_dnsmasq(&self.name, dnsmasq)
    }

    /// Attach an ACL to this bridge via `security.acls`.
    pub(crate) fn attach_acl(&self, acl_name: &str) -> Result<(), CommandError> {
        self.lxd.attach_acl_to_network(&self.name, acl_name)
    }

    /// Set the default egress action to `reject` on this bridge.
    pub(crate) fn set_default_egress_reject(&self) -> Result<(), CommandError> {
        self.lxd.set_network_egress_reject(&self.name)
    }
}

impl Drop for LxdBridge {
    fn drop(&mut self) {
        if self.keep {
            info!(bridge = %self.name, "keeping isolation bridge");
            return;
        }
        info!(bridge = %self.name, "destroying isolation bridge");
        if let Err(e) = self.lxd.delete_network(&self.name) {
            error!(bridge = %self.name, error = %e, "failed to delete bridge");
        }
    }
}
