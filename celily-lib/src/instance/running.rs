use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use super::{Instance, InstanceError, InstanceGuard};
use crate::backend::{InstanceBackend, NetworkBackend};
use crate::network::NetworkIsolation;

// ---------------------------------------------------------------------------
// Running state
// ---------------------------------------------------------------------------

/// Instance booted and ready to run commands.
pub struct Running<IB: InstanceBackend> {
    /// RAII guard -- deletes the instance on drop (unless keep).
    /// Declared before `isolation` for correct drop order.
    pub guard: InstanceGuard<IB>,
    /// Network isolation (proxy + bridge guard).
    pub isolation: NetworkIsolation,
    /// Proxy gateway IP.
    pub proxy_gateway: String,
    /// Proxy port.
    pub proxy_port: u16,
}

// ---------------------------------------------------------------------------
// Running -- exec
// ---------------------------------------------------------------------------

impl<IB: InstanceBackend, NB: NetworkBackend> Instance<IB, NB, Running<IB>> {
    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub async fn exec(
        &self,
        command: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&Path>,
    ) -> Result<i32, InstanceError<IB::Error>> {
        let proxy_url = format!(
            "http://{}:{}",
            self.state.proxy_gateway, self.state.proxy_port
        );
        let cwd = cwd.unwrap_or(&self.config.container_home);

        let code = self
            .instance_backend
            .exec(
                &self.config.name,
                command,
                env,
                cwd,
                self.config.exec_uid,
                self.config.exec_gid,
                Some(&self.config.container_home),
                Some(&proxy_url),
            )
            .await
            .map_err(|e| InstanceError::backend("failed to run lxc exec", e))?;

        Ok(code)
    }

    /// Argument vector (program first) that runs `command` inside the
    /// instance as the exec user, in `cwd`, with only `HOME` set: no proxy
    /// and no user environment. For handing the instance to another
    /// program, e.g. git's `ext::` transport.
    #[must_use]
    pub fn exec_argv(&self, command: &[String], cwd: &Path) -> Vec<OsString> {
        self.instance_backend.exec_argv(
            &self.config.name,
            command,
            &[("HOME", self.config.container_home.as_os_str())],
            cwd,
            self.config.exec_uid,
            self.config.exec_gid,
        )
    }
}
