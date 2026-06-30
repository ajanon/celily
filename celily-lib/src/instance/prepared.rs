use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use tracing::warn;

use super::{
    Initialized,
    Instance,
    InstanceError,
    InstanceGuard,
    InstanceKind,
    LaunchConfig,
    Running,
};
use crate::backend::{Device, InstanceBackend, InstanceConfig, NetworkBackend};
use crate::distro::DistroKind;
use crate::limits::Limits;
use crate::mount::Mount;
use crate::network::NetworkIsolation;
use crate::network::params::NetworkParams;
use crate::secrets::{SecretError, SecretProvider};

// ---------------------------------------------------------------------------
// Prepared state
// ---------------------------------------------------------------------------

/// No runtime state -- configuration has been assembled but the backend
/// instance has not been created yet.
pub struct Prepared;

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

#[bon::bon]
impl<IB: InstanceBackend, NB: NetworkBackend> Instance<IB, NB, Prepared> {
    #[builder(finish_fn = build)]
    #[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
    pub fn prepare(
        instance_backend: Arc<IB>,
        network_backend: Arc<NB>,
        image: String,
        name: Option<String>,
        bridge_name: Option<String>,
        #[builder(default)] kind: InstanceKind,
        distro: DistroKind,
        limits: Limits,
        network: NetworkParams,
        #[builder(default)] mounts: Vec<Mount>,
        #[builder(default)] extra_devices: Vec<Device>,
        secret_provider: Option<Box<dyn SecretProvider<Error = SecretError>>>,
        #[builder(default = 1000)] container_uid: u32,
        #[builder(default = 1000)] container_gid: u32,
        #[builder(default = 1000)] exec_uid: u32,
        #[builder(default = 1000)] exec_gid: u32,
        #[builder(default = PathBuf::from("/home/dev"))] container_home: PathBuf,
        raw_idmap: Option<String>,
        #[builder(default)] security_nesting: bool,
        #[builder(default = true)] secure_boot: bool,
        #[builder(default)] ephemeral: bool,
        #[builder(default)] keep: bool,
        #[builder(default)] description: String,
    ) -> Self {
        let name = name.unwrap_or_else(|| {
            let uuid = uuid::Uuid::new_v4().to_string();
            uuid[..8].to_string()
        });
        let bridge_name = bridge_name.unwrap_or_else(|| {
            let uuid = uuid::Uuid::new_v4();
            format!("br-{}", &uuid.to_string()[..8])
        });

        let config = LaunchConfig {
            image,
            name,
            bridge_name,
            kind,
            distro,
            mounts,
            extra_devices,
            container_uid,
            container_gid,
            exec_uid,
            exec_gid,
            container_home,
            raw_idmap,
            security_nesting,
            secure_boot,
            ephemeral,
            keep,
            description,
            limits,
            network,
        };

        Self {
            instance_backend,
            network_backend,
            secret_provider,
            config,
            state: Prepared,
        }
    }
}

// ---------------------------------------------------------------------------
// Prepared -> Initialized
// ---------------------------------------------------------------------------

impl<IB: InstanceBackend, NB: NetworkBackend> Instance<IB, NB, Prepared> {
    pub fn init(self) -> Result<Instance<IB, NB, Initialized<IB>>, InstanceError<IB::Error>> {
        // Build backend config and create the instance.
        let instance_config = InstanceConfig {
            image: self.config.image.clone(),
            ephemeral: self.config.ephemeral,
            vm: self.config.kind == InstanceKind::Vm,
            secure_boot: self.config.secure_boot,
            cpu: Some(self.config.limits.cpu),
            memory: Some(self.config.limits.memory.clone()),
            disk_size: Some(self.config.limits.disk.clone()),
            processes: Some(self.config.limits.processes),
            disk_priority: self.config.limits.disk_priority,
            memory_enforce: self.config.limits.memory_enforce.clone(),
            raw_idmap: self.config.raw_idmap.clone(),
            security_nesting: self.config.security_nesting,
        };

        self.instance_backend
            .create(&self.config.name, &instance_config)
            .map_err(|e| InstanceError::backend("failed to create instance", e))?;

        // RAII guard -- any failure from here on deletes the instance.
        let guard = InstanceGuard::new(
            self.config.name.clone(),
            Arc::clone(&self.instance_backend),
            self.config.keep,
        );

        // Add devices and set up network isolation in parallel.
        let (isolation, ca_cert) = thread::scope(|s| {
            let iso_handle = s.spawn({
                let nb = Arc::clone(&self.network_backend);
                let cbp = self
                    .config
                    .network
                    .to_create_bridge_params(self.config.keep);
                let provider = self.secret_provider.as_deref();
                let bridge = self.config.bridge_name.clone();
                let allow = self.config.network.allow.clone();
                let dns = self.config.network.dns;
                move || NetworkIsolation::setup(nb.as_ref(), &bridge, &cbp, &allow, dns, provider)
            });

            // Add mount devices while the bridge is being created.
            for (i, mount) in self.config.mounts.iter().enumerate() {
                if self.config.kind == InstanceKind::Vm && mount.source.is_file() {
                    warn!(
                        instance = %self.config.name,
                        source = %mount.source.display(),
                        "skipping file-based mount on VM (not supported by LXD/Incus)"
                    );
                    continue;
                }
                let dev_name = format!("mount{i}");
                let device = Device::Disk {
                    source: mount.source.clone(),
                    target: mount.target.clone(),
                    access: mount.access,
                };
                self.instance_backend
                    .add_device(&self.config.name, &dev_name, &device)
                    .map_err(|e| InstanceError::backend("failed to add disk device", e))?;
            }

            // Add pre-built devices (proxy, etc.) from the caller.
            for (i, device) in self.config.extra_devices.iter().enumerate() {
                let dev_name = format!("extra{i}");
                self.instance_backend
                    .add_device(&self.config.name, &dev_name, device)
                    .map_err(|e| InstanceError::backend("failed to add extra device", e))?;
            }

            // Set description if non-empty.
            if !self.config.description.is_empty() {
                self.instance_backend
                    .set_description(&self.config.name, &self.config.description)
                    .map_err(|e| InstanceError::backend("failed to set description", e))?;
            }

            let (isolation, ca_cert) = iso_handle.join().unwrap()?;
            Ok::<_, InstanceError<IB::Error>>((isolation, ca_cert))
        })?;

        // Attach instance to the isolation bridge.
        self.instance_backend
            .attach_to_bridge(
                &self.config.name,
                &self.config.bridge_name,
                self.config.limits.network_ingress.as_deref(),
                self.config.limits.network_egress.as_deref(),
            )
            .map_err(|e| InstanceError::backend("failed to attach instance to bridge", e))?;

        Ok(Instance {
            instance_backend: self.instance_backend,
            network_backend: self.network_backend,
            secret_provider: self.secret_provider,
            config: self.config,
            state: Initialized {
                guard,
                isolation,
                ca_cert,
            },
        })
    }

    pub fn launch(self) -> Result<Instance<IB, NB, Running<IB>>, InstanceError<IB::Error>> {
        self.init()?.start()
    }
}
