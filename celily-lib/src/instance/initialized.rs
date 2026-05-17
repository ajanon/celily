use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use tracing::info;

use crate::backend::{InstanceBackend, NetworkBackend};
use crate::network::NetworkIsolation;

use super::{Instance, InstanceError, InstanceGuard, Running};

// ---------------------------------------------------------------------------
// Initialized state
// ---------------------------------------------------------------------------

/// Instance created, devices attached, network isolation active.
/// Ready to be started.
pub struct Initialized<IB: InstanceBackend> {
    /// RAII guard -- deletes the instance on drop (unless keep).
    /// Declared before `isolation` so the instance is deleted before
    /// the bridge is torn down.
    pub guard: InstanceGuard<IB>,
    /// Network isolation (proxy + bridge guard).
    pub isolation: NetworkIsolation,
    /// PEM-encoded CA certificate for mitmproxy.
    pub ca_cert: String,
}

// ---------------------------------------------------------------------------
// Initialized -> Running
// ---------------------------------------------------------------------------

impl<IB: InstanceBackend, NB: NetworkBackend> Instance<IB, NB, Initialized<IB>> {
    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn start(self) -> Result<Instance<IB, NB, Running<IB>>, InstanceError<IB::Error>> {
        self.instance_backend
            .start(&self.config.name)
            .map_err(|e| InstanceError::backend("failed to start instance", e))?;

        let mut ready = false;
        for _ in 0..60 {
            match self
                .instance_backend
                .exec_stdout(&self.config.name, &["systemctl", "is-system-running"])
            {
                Ok(out) if out.trim() == "running" => { ready = true; break; }
                _ => {},
            }
            sleep(Duration::from_millis(500));
        }
        if !ready {
            return Err(InstanceError::Timeout { name: self.config.name.clone() });
        }

        let container_cert_path = format!(
            "{}/mitmproxy-ca-cert.pem",
            self.config.distro.ca_cert_anchors_dir(),
        );
        self.instance_backend.write_file(
            &self.config.name, self.state.ca_cert.as_bytes(), &container_cert_path,
            "0644", 0, 0,
        ).map_err(|e| InstanceError::backend("failed to push CA cert", e))?;

        self.instance_backend.exec_stdout(
            &self.config.name, &[self.config.distro.rebuild_trust_store_command()],
        ).map_err(|e| InstanceError::backend("failed to rebuild trust store", e))?;

        info!(instance = %self.config.name, "CA cert pushed, trust store rebuilt");

        {
            let mut parents = BTreeSet::new();
            for mount in &self.config.mounts {
                let mut current = mount.target.parent();
                while let Some(parent) = current {
                    if parent == self.config.container_home || !parent.starts_with(&self.config.container_home) {
                        break;
                    }
                    if self.config.mounts.iter().any(|m| m.target == parent) {
                        break;
                    }
                    parents.insert(parent.to_path_buf());
                    current = parent.parent();
                }
            }
            for parent in &parents {
                let owner = format!("{}:{}", self.config.container_uid, self.config.container_gid);
                self.instance_backend.exec_stdout(
                    &self.config.name,
                    &["chown", &owner, &parent.display().to_string()],
                ).map_err(|e| InstanceError::backend("failed to chown mount parent", e))?;
            }
        }

        let proxy_gateway = self.state.isolation.gateway_ip.to_string();
        let proxy_port = self.state.isolation.proxy.port;

        Ok(Instance {
            instance_backend: self.instance_backend,
            network_backend: self.network_backend,
            secret_provider: self.secret_provider,
            config: self.config,
            state: Running {
                guard: self.state.guard,
                isolation: self.state.isolation,
                proxy_gateway,
                proxy_port,
            },
        })
    }
}
