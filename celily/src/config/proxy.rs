use serde::Deserialize;

/// Configuration for a Unix socket proxy device.
///
/// Exposes a host Unix socket inside the instance through an LXD/Incus
/// proxy device.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyDevice {
    /// Host-side socket path in LXD/Incus proxy format (e.g.
    /// `unix:/run/user/1000/dbus-notifications.sock`).
    pub connect: String,
    /// Instance-side listen path (e.g. `unix:/run/dbus-notifications.sock`).
    pub listen: String,
    /// Which side the proxy listens on: `"instance"` (default) or `"host"`.
    #[serde(default = "default_bind")]
    pub bind: String,
    /// File mode for the socket inside the instance (e.g. `"0600"`).
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_bind() -> String {
    "instance".to_string()
}

fn default_mode() -> String {
    "0600".to_string()
}
