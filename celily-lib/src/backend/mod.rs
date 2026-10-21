use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::mount::AccessMode;

pub mod lxd;
pub mod mock;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// A device to attach to an instance.
#[derive(Debug, Clone)]
pub enum Device {
    /// Bind-mount a host directory or file into the instance.
    Disk {
        source: PathBuf,
        target: PathBuf,
        access: AccessMode,
    },
    /// Attach an existing custom storage volume of the backend's pool.
    Volume {
        /// Volume name, as in `lxc storage volume create <pool> <name>`.
        name: String,
        target: PathBuf,
        access: AccessMode,
    },
    /// Unix socket proxy, listening in the instance and connecting on the
    /// host.
    Proxy {
        /// LXD/Incus proxy connect string (e.g. `unix:/run/user/1000/sock`).
        connect: String,
        /// LXD/Incus proxy listen string (e.g. `unix:/run/sock`).
        listen: String,
        uid: u32,
        gid: u32,
        host_uid: u32,
        host_gid: u32,
        /// File mode for the socket inside the instance (e.g. `"0600"`).
        mode: String,
    },
}

/// Parameters for creating an instance.
#[derive(Debug, Clone, Default)]
pub struct InstanceConfig {
    pub image: String,
    pub ephemeral: bool,
    pub vm: bool,
    pub secure_boot: bool,
    pub cpu: Option<u32>,
    pub memory: Option<String>,
    pub disk_size: Option<String>,
    pub processes: Option<i32>,
    pub disk_priority: Option<u32>,
    pub memory_enforce: Option<String>,
    pub raw_idmap: Option<String>,
    pub security_nesting: bool,
}

// ---------------------------------------------------------------------------
// InstanceBackend
// ---------------------------------------------------------------------------

/// Manages the lifecycle of an isolated instance.
#[async_trait]
pub trait InstanceBackend: Send + Sync {
    /// The error type returned by all operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Create the instance without starting it.
    async fn create(&self, name: &str, config: &InstanceConfig) -> Result<(), Self::Error>;

    /// Boot the instance.
    async fn start(&self, name: &str) -> Result<(), Self::Error>;

    /// Delete the instance (forcefully).
    /// Synchronous because it is called from Drop.
    fn delete(&self, name: &str) -> Result<(), Self::Error>;

    /// Attach a device to the instance.
    async fn add_device(
        &self,
        name: &str,
        dev_name: &str,
        device: &Device,
    ) -> Result<(), Self::Error>;

    /// Override the default NIC to connect to the given bridge.
    async fn attach_to_bridge(
        &self,
        name: &str,
        bridge: &str,
        ingress: Option<&str>,
        egress: Option<&str>,
    ) -> Result<(), Self::Error>;

    /// Set the instance description.
    async fn set_description(&self, name: &str, desc: &str) -> Result<(), Self::Error>;

    /// Execute a command inside the instance, returning the exit code.
    async fn exec(
        &self,
        name: &str,
        cmd: &[String],
        env: &HashMap<String, String>,
        cwd: &Path,
        uid: u32,
        gid: u32,
        home: Option<&Path>,
        proxy_url: Option<&str>,
    ) -> Result<i32, Self::Error>;

    /// Execute a command inside the instance, capturing stdout.
    async fn exec_stdout(&self, name: &str, cmd: &[&str]) -> Result<String, Self::Error>;

    /// Build the argument vector (program first) that runs `cmd` inside the
    /// instance as `uid:gid` in `cwd`, with `env` applied in order.
    ///
    /// This is what [`exec`](Self::exec) runs. It is exposed so the
    /// invocation can be handed to another program, e.g. git's `ext::`
    /// transport.
    fn exec_argv(
        &self,
        name: &str,
        cmd: &[String],
        env: &[(&str, &OsStr)],
        cwd: &Path,
        uid: u32,
        gid: u32,
    ) -> Vec<OsString>;

    /// Write file content to a path inside the instance.
    async fn write_file(
        &self,
        name: &str,
        content: &[u8],
        path: &str,
        mode: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), Self::Error>;
}

// ---------------------------------------------------------------------------
// NetworkBackend
// ---------------------------------------------------------------------------

/// RAII guard that tears down a network bridge on drop.
pub trait BridgeGuard: Send + Sync {
    // Drop handles teardown.
}

/// A bridge's IPv4 gateway and subnet prefix length, as in `10.0.0.1/24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeSubnet {
    /// The gateway: the host's own address on the bridge.
    pub gateway: Ipv4Addr,
    /// Prefix length of the subnet, at most 32.
    pub prefix_len: u8,
}

impl BridgeSubnet {
    /// Parse `<gateway>/<prefix length>`, LXD's `ipv4.address` format.
    #[must_use]
    pub fn parse(cidr: &str) -> Option<Self> {
        let (gateway, prefix_len) = cidr.split_once('/')?;
        let prefix_len = prefix_len.parse().ok().filter(|&len| len <= 32)?;
        Some(Self {
            gateway: gateway.parse().ok()?,
            prefix_len,
        })
    }

    /// Whether `ip` can be an instance on this bridge: in the subnet, and
    /// not the gateway, which is the host itself.
    #[must_use]
    pub fn has_instance(&self, ip: IpAddr) -> bool {
        let IpAddr::V4(ip) = ip.to_canonical() else {
            return false;
        };
        let mask = u32::MAX
            .checked_shl(32 - u32::from(self.prefix_len))
            .unwrap_or(0);
        ip != self.gateway && (ip.to_bits() ^ self.gateway.to_bits()) & mask == 0
    }
}

/// Manages per-instance network isolation bridges.
#[async_trait]
pub trait NetworkBackend: Send + Sync {
    /// The error type returned by all operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Create an isolated bridge and return a guard that deletes it on drop,
    /// along with the bridge's subnet.
    async fn create_bridge(
        &self,
        name: &str,
        params: &CreateBridgeParams,
    ) -> Result<(Box<dyn BridgeGuard>, BridgeSubnet), Self::Error>;
}

/// A single TCP allow rule for the bridge egress ACL.
#[derive(Debug, Clone)]
pub struct TcpAllow {
    /// Destination IP address.
    pub host: std::net::IpAddr,
    /// Destination TCP ports.
    pub ports: Vec<u16>,
}

/// Parameters for creating an isolated network bridge.
#[derive(Debug, Clone)]
pub struct CreateBridgeParams {
    /// Whether to keep the bridge after the process exits.
    pub keep: bool,
    /// The TCP port the HTTP proxy listens on.
    pub proxy_port: u16,
    /// The UDP port the DNS proxy listens on.
    pub dns_port: u16,
    /// TCP allow rules for the bridge egress ACL.
    pub allow_tcp: Vec<TcpAllow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_subnet_has_its_instances_only() {
        let subnet = BridgeSubnet::parse("10.35.140.1/24").unwrap();
        let has = |ip: &str| subnet.has_instance(ip.parse().unwrap());
        assert!(has("10.35.140.2"));
        assert!(has("::ffff:10.35.140.200"));
        assert!(!has("10.35.140.1"), "the gateway is the host");
        assert!(!has("10.35.141.2"));
        assert!(!has("127.0.0.1"));
        assert!(!has("fd42::2"));

        let host_only = BridgeSubnet::parse("10.0.0.1/32").unwrap();
        assert!(!host_only.has_instance("10.0.0.2".parse().unwrap()));
        for invalid in ["10.0.0.1", "10.0.0.1/33", "fd42::1/64", "none"] {
            assert_eq!(BridgeSubnet::parse(invalid), None, "{invalid}");
        }
    }
}
