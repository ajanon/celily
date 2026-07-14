mod mitmproxy;
pub mod params;
pub mod rule;

use std::collections::HashMap;
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

    /// A secret could not be resolved.
    #[error(transparent)]
    Secret(#[from] SecretError),
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
    pub(crate) async fn setup<NB: NetworkBackend>(
        backend: &NB,
        bridge_name: &str,
        params: &CreateBridgeParams,
        allow: &[NetworkRule],
        dns: bool,
        provider: Option<&dyn SecretProvider<Error = SecretError>>,
    ) -> Result<(Self, String), NetworkError> {
        let (bridge_guard, gateway_ip) = backend
            .create_bridge(bridge_name, params)
            .await
            .map_err(|e| NetworkError::Backend(Box::new(e)))?;

        // Resolve auth secrets (each unique name once).
        let mut auth_secrets: HashMap<String, String> = HashMap::new();
        for rule in allow {
            if let NetworkRule::Http {
                auth: Some(auth), ..
            } = rule
                && !auth_secrets.contains_key(&auth.secret)
            {
                let p = provider.ok_or_else(|| SecretError::NoProvider {
                    secret: auth.secret.clone(),
                })?;
                let value = p.resolve(&auth.secret)?;
                auth_secrets.insert(auth.secret.clone(), value);
            }
        }

        let gateway_ip_str = gateway_ip.to_string();
        let bridge_name_owned = bridge_name.to_string();
        let allow_owned = allow.to_vec();
        let (proxy, ca_cert) = tokio::task::spawn_blocking(move || {
            MitmProxy::start(
                &bridge_name_owned,
                &gateway_ip_str,
                dns,
                &allow_owned,
                &auth_secrets,
            )
        })
        .await
        .unwrap()?;

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
