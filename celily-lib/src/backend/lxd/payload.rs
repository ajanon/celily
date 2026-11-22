//! The JSON document piped to `lxc init` / `incus create` on stdin.
//!
//! Both CLIs parse stdin as YAML into their `api.InstancePut`; JSON is
//! valid YAML for both parsers (go.yaml.in v2 for LXD, v4 for Incus).
//! `profiles` and `ephemeral` are left out: both come from command-line
//! flags, which win over stdin.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Every value is a string: the Go side unmarshals into
/// `map[string]string`. `BTreeMap` keeps the bytes deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct InstancePut {
    pub description: String,
    pub config: BTreeMap<String, String>,
    pub devices: BTreeMap<String, BTreeMap<String, String>>,
}

// -------------------------------------------------------------------
// tests
// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn sample() -> InstancePut {
        InstancePut {
            description: "celily stdin-InstancePut smoke test".into(),
            config: map(&[("limits.memory", "4GiB"), ("security.devlxd", "false")]),
            devices: BTreeMap::from([(
                "root".to_string(),
                map(&[
                    ("type", "disk"),
                    ("pool", "local"),
                    ("path", "/"),
                    ("size", "10GiB"),
                ]),
            )]),
        }
    }

    #[test]
    fn serializes_as_api_instance_put() {
        let json = serde_json::to_string(&sample()).unwrap();
        let expected = concat!(
            r#"{"description":"celily stdin-InstancePut smoke test","#,
            r#""config":{"limits.memory":"4GiB","security.devlxd":"false"},"#,
            r#""devices":{"root":"#,
            r#"{"path":"/","pool":"local","size":"10GiB","type":"disk"}}}"#
        );
        assert_eq!(json, expected);
    }

    #[test]
    fn parses_back_identically() {
        let json = serde_json::to_string(&sample()).unwrap();
        let back: InstancePut = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sample());
    }
}
