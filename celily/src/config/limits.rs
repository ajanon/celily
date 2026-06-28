use merge::Merge;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, Merge)]
#[serde(default)]
#[merge(strategy = super::merge_strategy::overwrite_some)]
pub struct Limits {
    pub cpu: Option<u32>,
    pub memory: Option<String>,
    pub disk: Option<String>,
    pub processes: Option<i32>,
    /// disk priority (0-10, 0 = lowest priority). Containers only.
    pub disk_priority: Option<u32>,
    /// Memory enforcement mode ("soft" or "hard"). Containers only.
    pub memory_enforce: Option<String>,
    /// Incoming network bandwidth limit (bit/s, e.g. "100Mbit").
    pub network_ingress: Option<String>,
    /// Outgoing network bandwidth limit (bit/s, e.g. "50Mbit").
    pub network_egress: Option<String>,
}
