pub mod guard;
pub mod initialized;
pub mod prepared;
pub mod running;

use std::path::PathBuf;
use std::sync::Arc;

pub use guard::InstanceGuard;
pub use initialized::Initialized;
pub use prepared::Prepared;
pub use running::Running;
use serde::Deserialize;
use thiserror::Error;

use crate::backend::{Device, InstanceBackend, NetworkBackend};
use crate::distro::DistroKind;
use crate::limits::Limits;
use crate::mount::Mount;
use crate::network::NetworkError;
use crate::network::params::NetworkParams;
use crate::secrets::{SecretError, SecretProvider};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during the instance lifecycle.
#[derive(Debug, Error)]
pub enum InstanceError<BE: std::error::Error + Send + Sync + 'static> {
    /// An operation on the instance backend failed.
    #[error("{context}: {source}")]
    Backend {
        context: &'static str,
        #[source]
        source: BE,
    },

    /// Network isolation setup failed.
    #[error("{0}")]
    Network(#[from] NetworkError),

    /// The instance did not become ready within the timeout.
    #[error("timed out waiting for instance '{name}' to be ready")]
    Timeout { name: String },
}

impl<BE: std::error::Error + Send + Sync + 'static> InstanceError<BE> {
    pub(crate) fn backend(context: &'static str, source: BE) -> Self {
        Self::Backend { context, source }
    }
}

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstanceKind {
    #[default]
    Container,
    Vm,
}

impl<'de> Deserialize<'de> for InstanceKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        bool::deserialize(deserializer).map(|v| {
            if v {
                InstanceKind::Vm
            } else {
                InstanceKind::Container
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemState {
    Starting,
    Running,
    Degraded,
}

impl SystemState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Degraded => "degraded",
        }
    }
}

// ---------------------------------------------------------------------------
// Instance config
// ---------------------------------------------------------------------------

/// Configuration for a single instance launch.
///
/// All fields are set at prepare time and do not change across states.
/// Bundled to eliminate copy-paste in state transitions.
pub(crate) struct LaunchConfig {
    pub(crate) image: String,
    pub(crate) name: String,
    pub(crate) bridge_name: String,
    pub(crate) kind: InstanceKind,
    pub(crate) distro: DistroKind,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) extra_devices: Vec<Device>,
    pub(crate) container_uid: u32,
    pub(crate) container_gid: u32,
    pub(crate) exec_uid: u32,
    pub(crate) exec_gid: u32,
    pub(crate) container_home: PathBuf,
    pub(crate) raw_idmap: Option<String>,
    pub(crate) security_nesting: bool,
    pub(crate) secure_boot: bool,
    pub(crate) ephemeral: bool,
    pub(crate) keep: bool,
    pub(crate) description: String,
    pub(crate) limits: Limits,
    pub(crate) network: NetworkParams,
}

// ---------------------------------------------------------------------------
// Instance
// ---------------------------------------------------------------------------

pub struct Instance<IB: InstanceBackend, NB: NetworkBackend, S = Prepared> {
    pub(crate) instance_backend: Arc<IB>,
    pub(crate) network_backend: Arc<NB>,
    pub(crate) secret_provider: Option<Box<dyn SecretProvider<Error = SecretError>>>,
    pub(crate) config: LaunchConfig,
    pub(crate) state: S,
}
