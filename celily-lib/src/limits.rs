use bon::Builder;

/// Resource limits for an isolated environment.
///
/// Core fields (`cpu`, `memory`, `disk`, `processes`) are required
/// and have builder defaults. The remaining fields are optional
/// container-specific limits.
#[derive(Debug, Clone, Builder)]
pub struct Limits {
    /// CPU core limit.
    #[builder(default = 2)]
    pub cpu: u32,

    /// Memory limit (e.g. "4GiB", "512MiB").
    #[builder(default = String::from("4GiB"))]
    pub memory: String,

    /// Root disk size (e.g. "4GiB", "20GiB").
    #[builder(default = String::from("4GiB"))]
    pub disk: String,

    /// Maximum number of processes (-1 for unlimited).
    #[builder(default = 1024)]
    pub processes: i32,

    /// Disk I/O priority (0-10). Containers only.
    pub disk_priority: Option<u32>,

    /// Memory enforcement mode ("soft" or "hard"). Containers only.
    pub memory_enforce: Option<String>,

    /// Incoming network bandwidth limit (e.g. "100Mbit").
    pub network_ingress: Option<String>,

    /// Outgoing network bandwidth limit (e.g. "50Mbit").
    pub network_egress: Option<String>,
}
