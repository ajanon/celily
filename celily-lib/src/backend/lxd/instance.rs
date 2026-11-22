use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::Path;

use async_trait::async_trait;

use super::{LxcBackend, payload};
use crate::backend::{Device, ExecStdio, InstanceConfig};
use crate::command::{AsyncCommandExt, CommandError, CommandExt};
use crate::mount::AccessMode;

impl LxcBackend {
    /// The `api.InstancePut` projection of an [`InstanceConfig`]:
    /// everything `lxc init` takes through stdin -- description, config
    /// keys, and all devices including the root disk.
    ///
    /// The root disk rides in the payload, so `create` passes no
    /// `--storage`: the CLIs overwrite a stdin root device when the flag
    /// is given, and it triggers an extra `GetStoragePool` RPC on top.
    fn instance_put(&self, config: &InstanceConfig) -> payload::InstancePut {
        let mut config_map: BTreeMap<String, String> = BTreeMap::new();
        if config.vm && !config.secure_boot {
            config_map.insert("boot.mode".into(), "uefi-nosecureboot".into());
        }
        if let Some(n) = config.cpu {
            config_map.insert("limits.cpu".into(), n.to_string());
        }
        if let Some(ref m) = config.memory {
            config_map.insert("limits.memory".into(), m.clone());
        }
        if let Some(p) = config.processes {
            if config.vm {
                tracing::warn!("limits.processes is ignored on VMs");
            } else {
                config_map.insert("limits.processes".into(), p.to_string());
            }
        }
        if let Some(prio) = config.disk_priority {
            if config.vm {
                tracing::warn!("limits.disk.priority is ignored on VMs");
            } else {
                config_map.insert("limits.disk.priority".into(), prio.to_string());
            }
        }
        if let Some(ref mode) = config.memory_enforce {
            if config.vm {
                tracing::warn!("limits.memory.enforce is ignored on VMs");
            } else {
                config_map.insert("limits.memory.enforce".into(), mode.clone());
            }
        }
        if let Some(ref idmap) = config.raw_idmap {
            config_map.insert("raw.idmap".into(), idmap.clone());
        }
        if config.security_nesting {
            if config.vm {
                tracing::warn!("security.nesting is ignored on VMs");
            } else {
                config_map.insert("security.nesting".into(), "true".into());
            }
        }
        config_map.insert("security.devlxd".into(), "false".into());

        let mut root: BTreeMap<String, String> = BTreeMap::new();
        root.insert("type".into(), "disk".into());
        root.insert("pool".into(), self.pool.clone());
        root.insert("path".into(), "/".into());
        if let Some(ref size) = config.disk_size {
            root.insert("size".into(), size.clone());
        }

        let mut devices: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        devices.insert("root".into(), root);
        for (dev_name, device) in &config.devices {
            devices.insert(dev_name.clone(), self.device_map(device));
        }

        payload::InstancePut {
            description: config.description.clone(),
            config: config_map,
            devices,
        }
    }

    /// The `map[string]string` form of a device.
    fn device_map(&self, device: &Device) -> BTreeMap<String, String> {
        let mut map: BTreeMap<String, String> = match device {
            Device::Disk { source, .. } => {
                let mut map = BTreeMap::new();
                map.insert("type".into(), "disk".into());
                map.insert("source".into(), source.display().to_string());
                map
            },
            Device::Volume { name, .. } => {
                let mut map = BTreeMap::new();
                map.insert("type".into(), "disk".into());
                map.insert("pool".into(), self.pool.clone());
                map.insert("source".into(), name.clone());
                map
            },
            Device::Proxy {
                connect,
                listen,
                uid,
                gid,
                host_uid,
                host_gid,
                mode,
            } => {
                let mut map = BTreeMap::new();
                map.insert("type".into(), "proxy".into());
                map.insert("bind".into(), "instance".into());
                map.insert("connect".into(), connect.clone());
                map.insert("listen".into(), listen.clone());
                map.insert("mode".into(), mode.clone());
                map.insert("uid".into(), uid.to_string());
                map.insert("gid".into(), gid.to_string());
                map.insert("security.uid".into(), host_uid.to_string());
                map.insert("security.gid".into(), host_gid.to_string());
                map
            },
        };
        if let Device::Disk { target, access, .. } | Device::Volume { target, access, .. } = device
        {
            map.insert("path".into(), target.display().to_string());
            if *access == AccessMode::ReadOnly {
                map.insert("readonly".into(), "true".into());
            }
        }
        map
    }
}

#[async_trait]
impl crate::backend::InstanceBackend for LxcBackend {
    type Error = CommandError;

    async fn create(&self, name: &str, config: &InstanceConfig) -> Result<(), Self::Error> {
        self.ensure_project().await?;

        let mut cmd = self.lxc_project_command_async();
        cmd.args(["init", &config.image, name]);

        // No profile: the daemon's default profile could add a NIC that
        // bypasses the isolation bridge, disks, or raw.lxc. Every device
        // is celily's own, and they all arrive through the stdin payload.
        cmd.arg("--no-profiles");
        if config.ephemeral {
            cmd.arg("--ephemeral");
        }
        if config.vm {
            cmd.arg("--vm");
        }

        let payload = self.instance_put(config);
        let json = serde_json::to_string(&payload).expect("serializing InstancePut cannot fail");
        tracing::trace!(instance = name, "lxc init payload: {json}");

        cmd.run_with_stdin(json.as_bytes()).await?;

        Ok(())
    }

    async fn start(&self, name: &str) -> Result<(), Self::Error> {
        self.lxc_project_command_async()
            .args(["start", name])
            .run()
            .await
    }

    fn delete(&self, name: &str) -> Result<(), Self::Error> {
        self.lxc_project_command()
            .args(["delete", "--force", name])
            .run()
    }

    async fn attach_to_bridge(
        &self,
        name: &str,
        bridge_name: &str,
        network_ingress: Option<&str>,
        network_egress: Option<&str>,
    ) -> Result<(), Self::Error> {
        let mut args = vec![
            "config".to_string(),
            "device".to_string(),
            "add".to_string(),
            name.to_string(),
            "eth0".to_string(),
            "nic".to_string(),
            "nictype=bridged".to_string(),
            format!("parent={bridge_name}"),
        ];
        if let Some(ingress) = network_ingress {
            args.push(format!("limits.ingress={ingress}"));
        }
        if let Some(egress) = network_egress {
            args.push(format!("limits.egress={egress}"));
        }
        self.lxc_project_command_async().args(&args).run().await
    }

    async fn exec(
        &self,
        name: &str,
        command: &[String],
        env: &HashMap<String, String>,
        cwd: &Path,
        uid: u32,
        gid: u32,
        home: Option<&Path>,
        proxy_url: Option<&str>,
        stdio: ExecStdio,
    ) -> Result<i32, Self::Error> {
        let mut env_pairs: Vec<(&str, &OsStr)> = env
            .iter()
            .map(|(k, v)| (k.as_str(), OsStr::new(v)))
            .collect();
        if let Some(h) = home {
            env_pairs.push(("HOME", h.as_os_str()));
        }
        if let Some(proxy) = proxy_url {
            env_pairs.extend([
                ("HTTP_PROXY", OsStr::new(proxy)),
                ("HTTPS_PROXY", OsStr::new(proxy)),
                ("NO_PROXY", OsStr::new("")),
                ("NODE_USE_ENV_PROXY", OsStr::new("1")),
                ("NODE_USE_SYSTEM_CA", OsStr::new("1")),
            ]);
        }

        let mut cmd = tokio::process::Command::from(
            self.exec_command(name, command, &env_pairs, cwd, uid, gid),
        );
        if stdio == ExecStdio::Setup {
            // No terminal either: lxc exec allocates one only when its stdin
            // and stdout both are.
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::io::stderr());
        }
        let status = cmd.status().await?;
        let code = status.code().unwrap_or(1);
        Ok(code)
    }

    async fn exec_stdout(&self, name: &str, command: &[&str]) -> Result<String, Self::Error> {
        let mut cmd = self.lxc_project_command_async();
        cmd.args(["exec", name, "--"]);
        cmd.args(command);
        cmd.run_stdout().await
    }

    fn exec_argv(
        &self,
        name: &str,
        cmd: &[String],
        env: &[(&str, &OsStr)],
        cwd: &Path,
        uid: u32,
        gid: u32,
    ) -> Vec<OsString> {
        let command = self.exec_command(name, cmd, env, cwd, uid, gid);
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(OsStr::to_os_string)
            .collect()
    }

    async fn write_file(
        &self,
        name: &str,
        content: &[u8],
        path: &str,
        mode: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), Self::Error> {
        use tokio::io::AsyncWriteExt;

        let dest = format!("{name}/{path}");
        let mut child = self
            .lxc_project_command_async()
            .args([
                "file",
                "push",
                "-",
                &dest,
                "--create-dirs",
                "--mode",
                mode,
                "--uid",
                &uid.to_string(),
                "--gid",
                &gid.to_string(),
            ])
            .stdin(std::process::Stdio::piped())
            // lxc's progress output: celily's stdout is the command's.
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(CommandError::Io)?;

        child
            .stdin
            .take()
            .ok_or_else(|| {
                CommandError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stdin pipe failed",
                ))
            })?
            .write_all(content)
            .await
            .map_err(CommandError::Io)?;

        let status = child.wait().await.map_err(CommandError::Io)?;
        if !status.success() {
            return Err(CommandError::NonZero {
                argv: format!("{} file push ... {dest}", self.binary),
                code: status.code(),
                stderr: None,
            });
        }
        Ok(())
    }
}

impl LxcBackend {
    /// Shared builder behind `exec` and `exec_argv`, so the argv handed to
    /// other programs is exactly the one celily runs itself.
    fn exec_command(
        &self,
        name: &str,
        cmd: &[String],
        env: &[(&str, &OsStr)],
        cwd: &Path,
        uid: u32,
        gid: u32,
    ) -> std::process::Command {
        let mut command = self.lxc_project_command();
        command
            .arg("exec")
            .arg(name)
            .arg("--user")
            .arg(uid.to_string())
            .arg("--group")
            .arg(gid.to_string())
            .arg("--cwd")
            .arg(cwd);
        for (key, value) in env {
            let mut pair = OsString::from(key);
            pair.push("=");
            pair.push(value);
            command.arg("--env").arg(pair);
        }
        command.arg("--").args(cmd);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::InstanceBackend;

    #[test]
    fn exec_argv_matches_lxc_exec_layout() {
        let mut backend = LxcBackend::incus();
        backend.project = Some("celily".into());
        let argv = backend.exec_argv(
            "inst",
            &["git".into(), "upload-pack".into()],
            &[("HOME", OsStr::new("/home/dev")), ("A", OsStr::new("b=c"))],
            Path::new("/home/dev"),
            1000,
            1001,
        );
        let expected = [
            "incus",
            "--project",
            "celily",
            "exec",
            "inst",
            "--user",
            "1000",
            "--group",
            "1001",
            "--cwd",
            "/home/dev",
            "--env",
            "HOME=/home/dev",
            "--env",
            "A=b=c",
            "--",
            "git",
            "upload-pack",
        ];
        assert_eq!(argv, expected.map(OsString::from));
    }

    #[test]
    fn volume_is_a_disk_of_the_backend_pool() {
        let mut backend = LxcBackend::lxd();
        backend.pool = "zfs".into();
        let volume = |access| Device::Volume {
            name: "pi-sessions".into(),
            target: "/home/dev/.pi/agent/sessions".into(),
            access,
        };
        let rw = [
            ("path", "/home/dev/.pi/agent/sessions"),
            ("pool", "zfs"),
            ("source", "pi-sessions"),
            ("type", "disk"),
        ];
        let as_map = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(
            backend.device_map(&volume(AccessMode::ReadWrite)),
            as_map(&rw)
        );
        assert_eq!(
            backend.device_map(&volume(AccessMode::ReadOnly)),
            as_map(&[&rw[..], &[("readonly", "true")]].concat()),
        );
    }

    #[test]
    fn instance_put_bundles_root_config_and_devices() {
        let backend = LxcBackend::lxd();
        let config = InstanceConfig {
            image: "celily".into(),
            ephemeral: true,
            vm: false,
            secure_boot: true,
            cpu: Some(2),
            memory: Some("4GiB".into()),
            disk_size: Some("10GiB".into()),
            processes: Some(1024),
            disk_priority: Some(5),
            memory_enforce: Some("hard".into()),
            raw_idmap: Some("uid 1000 1000".into()),
            security_nesting: true,
            devices: vec![(
                "mount0".into(),
                Device::Disk {
                    source: "/tmp/src".into(),
                    target: "/home/dev/src".into(),
                    access: AccessMode::ReadOnly,
                },
            )],
            description: "celily run".into(),
        };
        let put = backend.instance_put(&config);

        assert_eq!(put.description, "celily run");
        assert_eq!(
            put.config.get("raw.idmap").map(String::as_str),
            Some("uid 1000 1000")
        );
        assert_eq!(
            put.config.get("security.devlxd").map(String::as_str),
            Some("false")
        );
        assert_eq!(put.config.get("boot.mode"), None, "secure_boot stays on");
        assert_eq!(
            put.devices
                .get("root")
                .map(|root| root.get("size").map(String::as_str)),
            Some(Some("10GiB"))
        );
        let mount0 = put.devices.get("mount0").unwrap();
        assert_eq!(mount0.get("readonly").map(String::as_str), Some("true"));
        assert_eq!(mount0.get("source").map(String::as_str), Some("/tmp/src"));
    }
}
