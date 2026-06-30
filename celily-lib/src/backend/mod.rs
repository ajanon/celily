use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use crate::mount::AccessMode;

pub mod lxd;
pub mod mock;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub enum ProxyBind {
    #[default]
    Instance,
    Host,
}

/// A device to attach to an instance.
#[derive(Debug, Clone)]
pub enum Device {
    /// Bind-mount a host directory or file into the instance.
    Disk {
        source: PathBuf,
        target: PathBuf,
        access: AccessMode,
    },
    /// Unix socket proxy (e.g. for desktop notifications).
    Proxy {
        socket_path: String,
        listen_path: String,
        uid: u32,
        gid: u32,
        host_uid: u32,
        host_gid: u32,
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
pub trait InstanceBackend: Send + Sync {
    /// The error type returned by all operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Create the instance without starting it.
    fn create(&self, name: &str, config: &InstanceConfig) -> Result<(), Self::Error>;

    /// Boot the instance.
    fn start(&self, name: &str) -> Result<(), Self::Error>;

    /// Delete the instance (forcefully).
    fn delete(&self, name: &str) -> Result<(), Self::Error>;

    /// Attach a device to the instance.
    fn add_device(&self, name: &str, dev_name: &str, device: &Device) -> Result<(), Self::Error>;

    /// Override the default NIC to connect to the given bridge.
    fn attach_to_bridge(
        &self,
        name: &str,
        bridge: &str,
        ingress: Option<&str>,
        egress: Option<&str>,
    ) -> Result<(), Self::Error>;

    /// Set the instance description.
    fn set_description(&self, name: &str, desc: &str) -> Result<(), Self::Error>;

    /// Execute a command inside the instance, returning the exit code.
    fn exec(
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
    fn exec_stdout(&self, name: &str, cmd: &[&str]) -> Result<String, Self::Error>;

    /// Write file content to a path inside the instance.
    fn write_file(
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

/// Manages per-instance network isolation bridges.
pub trait NetworkBackend: Send + Sync {
    /// The error type returned by all operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Create an isolated bridge and return a guard that deletes it on drop,
    /// along with the bridge's gateway IP.
    fn create_bridge(
        &self,
        name: &str,
        params: &CreateBridgeParams,
    ) -> Result<(Box<dyn BridgeGuard>, IpAddr), Self::Error>;
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
    /// Whether to enable DNS filtering through the proxy.
    pub dns: bool,
    /// The TCP port the HTTP proxy listens on.
    pub proxy_port: u16,
    /// The port the DNS listener uses.
    pub dns_port: u16,
    /// TCP allow rules for the bridge egress ACL.
    pub allow_tcp: Vec<TcpAllow>,
}
