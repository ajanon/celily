use std::path::PathBuf;
use std::sync::Arc;

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
    /// Mounts, then caller-built devices. File mounts are skipped on VMs;
    /// names keep their index, so a skip leaves a gap.
    fn assemble_devices(&self) -> Vec<(String, Device)> {
        let mut devices =
            Vec::with_capacity(self.config.mounts.len() + self.config.extra_devices.len());
        for (i, mount) in self.config.mounts.iter().enumerate() {
            if self.config.kind == InstanceKind::Vm && mount.source.is_file() {
                warn!(
                    instance = %self.config.name,
                    source = %mount.source.display(),
                    "skipping file-based mount on VM (not supported by LXD/Incus)"
                );
                continue;
            }
            devices.push((
                format!("mount{i}"),
                Device::Disk {
                    source: mount.source.clone(),
                    target: mount.target.clone(),
                    access: mount.access,
                },
            ));
        }
        for (i, device) in self.config.extra_devices.iter().enumerate() {
            devices.push((format!("extra{i}"), device.clone()));
        }
        devices
    }

    pub async fn init(self) -> Result<Instance<IB, NB, Initialized<IB>>, InstanceError<IB::Error>> {
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
            devices: self.assemble_devices(),
            description: self.config.description.clone(),
        };

        self.instance_backend
            .create(&self.config.name, &instance_config)
            .await
            .map_err(|e| InstanceError::backend("failed to create instance", e))?;

        // RAII guard -- any failure from here on deletes the instance.
        let guard = InstanceGuard::new(
            self.config.name.clone(),
            Arc::clone(&self.instance_backend),
            self.config.keep,
        );

        let cbp = self
            .config
            .network
            .to_create_bridge_params(self.config.keep);
        let (isolation, ca_cert) = NetworkIsolation::setup(
            self.network_backend.as_ref(),
            &self.config.bridge_name,
            &cbp,
            &self.config.network.allow,
            self.secret_provider.as_deref(),
        )
        .await?;

        // Attach instance to the isolation bridge.
        self.instance_backend
            .attach_to_bridge(
                &self.config.name,
                &self.config.bridge_name,
                self.config.limits.network_ingress.as_deref(),
                self.config.limits.network_egress.as_deref(),
            )
            .await
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

    pub async fn launch(self) -> Result<Instance<IB, NB, Running<IB>>, InstanceError<IB::Error>> {
        self.init().await?.start().await
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::backend::mock::{MockInstanceBackend, MockNetworkBackend};
    use crate::distro::DistroKind;
    use crate::limits::Limits;
    use crate::mount::AccessMode;
    use crate::network::params::NetworkParams;

    fn instance(
        kind: InstanceKind,
        mounts: Vec<Mount>,
    ) -> Instance<MockInstanceBackend, MockNetworkBackend, Prepared> {
        Instance {
            instance_backend: Arc::new(MockInstanceBackend::new()),
            network_backend: Arc::new(MockNetworkBackend::new()),
            secret_provider: None,
            config: LaunchConfig {
                image: "celily".into(),
                name: "inst".into(),
                bridge_name: "br-test".into(),
                kind,
                distro: DistroKind::Arch,
                mounts,
                extra_devices: vec![Device::Proxy {
                    connect: "unix:/run/user/1000/x.sock".into(),
                    listen: "unix:/home/dev/x.sock".into(),
                    uid: 1000,
                    gid: 1000,
                    host_uid: 1000,
                    host_gid: 1000,
                    mode: "0600".into(),
                }],
                container_uid: 1000,
                container_gid: 1000,
                exec_uid: 1000,
                exec_gid: 1000,
                container_home: "/home/dev".into(),
                raw_idmap: None,
                security_nesting: false,
                secure_boot: true,
                ephemeral: true,
                keep: false,
                description: "celily run".into(),
                limits: Limits::builder().build(),
                network: NetworkParams::builder().build(),
            },
            state: Prepared,
        }
    }

    fn dir_and_file_mounts(dir: &Path, file: &Path) -> Vec<Mount> {
        fs::create_dir(dir).unwrap();
        fs::write(file, b"file").unwrap();
        vec![
            Mount {
                source: dir.to_path_buf(),
                target: "/home/dev/dir".into(),
                access: AccessMode::ReadOnly,
            },
            Mount {
                source: file.to_path_buf(),
                target: "/home/dev/file".into(),
                access: AccessMode::ReadOnly,
            },
        ]
    }

    #[test]
    fn assemble_devices_names_mounts_then_extras() {
        let tmp = tempfile::tempdir().unwrap();
        let mounts = dir_and_file_mounts(&tmp.path().join("dir"), &tmp.path().join("file"));
        let inst = instance(InstanceKind::Container, mounts);

        let devices = inst.assemble_devices();
        assert_eq!(
            devices,
            vec![
                (
                    "mount0".into(),
                    Device::Disk {
                        source: tmp.path().join("dir"),
                        target: "/home/dev/dir".into(),
                        access: AccessMode::ReadOnly,
                    }
                ),
                (
                    "mount1".into(),
                    Device::Disk {
                        source: tmp.path().join("file"),
                        target: "/home/dev/file".into(),
                        access: AccessMode::ReadOnly,
                    }
                ),
                ("extra0".into(), inst.config.extra_devices[0].clone()),
            ]
        );
    }

    #[test]
    fn assemble_devices_skips_file_mounts_on_vms_and_keeps_names() {
        let tmp = tempfile::tempdir().unwrap();
        let mounts = dir_and_file_mounts(&tmp.path().join("dir"), &tmp.path().join("file"));
        let inst = instance(InstanceKind::Vm, mounts);

        let devices = inst.assemble_devices();
        assert_eq!(
            devices,
            vec![
                (
                    "mount0".into(),
                    Device::Disk {
                        source: tmp.path().join("dir"),
                        target: "/home/dev/dir".into(),
                        access: AccessMode::ReadOnly,
                    }
                ),
                ("extra0".into(), inst.config.extra_devices[0].clone()),
            ]
        );
    }
}
