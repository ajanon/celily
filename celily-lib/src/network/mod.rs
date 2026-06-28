mod mitmproxy;
pub mod params;
pub mod rule;

use std::net::IpAddr;

use mitmproxy::MitmProxy;
pub use mitmproxy::MitmProxyError;
pub use params::NetworkParams;
pub use rule::{AuthConfig, NetworkRule, QuotaConfig};

use crate::backend::{BridgeGuard, CreateBridgeParams, NetworkBackend};
use crate::secrets::{SecretError, SecretProvider};

/// Errors that can occur during network isolation setup.
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// The backend failed to create the isolation bridge.
    #[error("backend error: {0}")]
    Backend(Box<dyn std::error::Error + Send + Sync>),

    /// The mitmproxy failed to start.
    #[error(transparent)]
    Proxy(#[from] MitmProxyError),
}

/// Wrapper holding the RAII guards for network isolation.
///
/// Drop order (declaration order): proxy dies first (connections stop),
/// then the bridge guard is dropped (tearing down the bridge and ACL).
/// Callers should drop the `InstanceGuard` *before* dropping this guard
/// so the instance is gone before the bridge is deleted.
pub struct NetworkIsolation {
    pub proxy: MitmProxy,
    pub gateway_ip: IpAddr,
    pub bridge_guard: Box<dyn BridgeGuard>,
}

impl NetworkIsolation {
    /// Full setup. Creates the isolation bridge via the backend,
    /// starts mitmproxy, and returns the isolation guard and the
    /// PEM-encoded CA certificate.
    pub(crate) fn setup<NB: NetworkBackend>(
        backend: &NB,
        bridge_name: &str,
        params: &CreateBridgeParams,
        allow: &[NetworkRule],
        dns: bool,
        provider: Option<&dyn SecretProvider<Error = SecretError>>,
    ) -> Result<(Self, String), NetworkError> {
        let (bridge_guard, gateway_ip) = backend
            .create_bridge(bridge_name, params)
            .map_err(|e| NetworkError::Backend(Box::new(e)))?;

        let (proxy, ca_cert) =
            MitmProxy::start(bridge_name, &gateway_ip.to_string(), dns, allow, provider)?;

        Ok((
            Self {
                proxy,
                gateway_ip,
                bridge_guard,
            },
            ca_cert,
        ))
    }
}
