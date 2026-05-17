use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
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

impl Limits {
    /// Merge two `Limits` values. Each field uses the profile value when
    /// set, falling back to the default.
    pub(super) fn merge(default: &Self, profile: &Self) -> Self {
        Self {
            cpu: profile.cpu.or(default.cpu),
            memory: profile.memory.clone().or_else(|| default.memory.clone()),
            disk: profile.disk.clone().or_else(|| default.disk.clone()),
            processes: profile.processes.or(default.processes),
            disk_priority: profile.disk_priority.or(default.disk_priority),
            memory_enforce: profile
                .memory_enforce
                .clone()
                .or_else(|| default.memory_enforce.clone()),
            network_ingress: profile
                .network_ingress
                .clone()
                .or_else(|| default.network_ingress.clone()),
            network_egress: profile
                .network_egress
                .clone()
                .or_else(|| default.network_egress.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits_with_cpu(cpu: u32) -> Limits {
        Limits {
            cpu: Some(cpu),
            ..Default::default()
        }
    }

    fn limits_with_all() -> Limits {
        Limits {
            cpu: Some(2),
            memory: Some("2GiB".into()),
            disk: Some("5GiB".into()),
            processes: Some(512),
            disk_priority: Some(5),
            memory_enforce: Some("soft".into()),
            network_ingress: None,
            network_egress: None,
        }
    }

    #[test]
    fn profile_overrides_default() {
        let profile = limits_with_cpu(4);
        let default = limits_with_cpu(2);
        let merged = Limits::merge(&default, &profile);
        assert_eq!(merged.cpu, Some(4));
    }

    #[test]
    fn falls_back_to_default() {
        let profile = Limits::default();
        let default = limits_with_cpu(2);
        let merged = Limits::merge(&default, &profile);
        assert_eq!(merged.cpu, Some(2));
    }

    #[test]
    fn both_none() {
        let merged = Limits::merge(&Limits::default(), &Limits::default());
        assert_eq!(merged.cpu, None);
        assert_eq!(merged.memory, None);
        assert_eq!(merged.disk, None);
        assert_eq!(merged.processes, None);
        assert_eq!(merged.disk_priority, None);
        assert_eq!(merged.memory_enforce, None);
        assert_eq!(merged.network_ingress, None);
        assert_eq!(merged.network_egress, None);
    }

    #[test]
    fn all_fields_profile_wins() {
        let mut profile = limits_with_all();
        profile.network_ingress = Some("100Mbit".into());
        profile.network_egress = Some("50Mbit".into());
        let default = Limits {
            cpu: Some(8),
            memory: Some("8GiB".into()),
            disk: Some("16GiB".into()),
            processes: Some(2048),
            disk_priority: Some(1),
            memory_enforce: Some("hard".into()),
            network_ingress: Some("10Mbit".into()),
            network_egress: Some("5Mbit".into()),
        };
        let merged = Limits::merge(&default, &profile);
        assert_eq!(merged.cpu, profile.cpu);
        assert_eq!(merged.memory, profile.memory);
        assert_eq!(merged.disk, profile.disk);
        assert_eq!(merged.processes, profile.processes);
        assert_eq!(merged.disk_priority, profile.disk_priority);
        assert_eq!(merged.memory_enforce, profile.memory_enforce);
        assert_eq!(merged.network_ingress.as_deref(), Some("100Mbit"));
        assert_eq!(merged.network_egress.as_deref(), Some("50Mbit"));
    }

    #[test]
    fn all_fields_default_fallback() {
        let profile = Limits::default();
        let mut default = limits_with_all();
        default.network_ingress = Some("100Mbit".into());
        default.network_egress = Some("50Mbit".into());
        let merged = Limits::merge(&default, &profile);
        assert_eq!(merged.cpu, default.cpu);
        assert_eq!(merged.memory, default.memory);
        assert_eq!(merged.disk, default.disk);
        assert_eq!(merged.processes, default.processes);
        assert_eq!(merged.disk_priority, default.disk_priority);
        assert_eq!(merged.memory_enforce, default.memory_enforce);
        assert_eq!(merged.network_ingress.as_deref(), Some("100Mbit"));
        assert_eq!(merged.network_egress.as_deref(), Some("50Mbit"));
    }
}
