use std::path::Path;

use anyhow::Context;
use celily_lib::backend::{Device, InstanceBackend};
use tracing::info;

/// If the notification proxy socket exists on the host, add an LXD
/// proxy device so processes inside the container can send desktop
/// notifications through it.
///
/// The socket is created by `xdg-dbus-proxy-notifications.service`.
/// Returns `Ok(())` if the device was added or the socket doesn't exist;
/// `Err` only if the socket exists but the device could not be added.
pub fn add_proxy<B: InstanceBackend>(
    backend: &B,
    instance_name: &str,
    container_uid: u32,
    container_gid: u32,
) -> anyhow::Result<()> {
    let host_uid = nix::unistd::Uid::current().as_raw();
    let host_gid = nix::unistd::Gid::current().as_raw();
    let socket_path = format!("/run/user/{host_uid}/dbus-notifications.sock");

    if !Path::new(&socket_path).exists() {
        return Ok(());
    }

    info!(
        instance = %instance_name,
        socket = %socket_path,
        "attaching notification proxy"
    );

    // Use /run inside the container - it's always present once the
    // container boots (systemd mounts it early), unlike /run/user/<uid>
    // which pam_systemd creates at login and may race with device
    // activation.
    let listen_path = "/run/dbus-notifications.sock";

    let device = Device::Proxy {
        socket_path: socket_path.clone(),
        listen_path: listen_path.to_string(),
        uid: container_uid,
        gid: container_gid,
        host_uid,
        host_gid,
    };
    backend.add_device(instance_name, "notifications", &device)
        .context("failed to add notification proxy device")?;

    info!("notification proxy device attached at {listen_path}");
    Ok(())
}
