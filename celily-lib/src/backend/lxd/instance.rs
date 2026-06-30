use std::collections::HashMap;
use std::path::Path;

use super::LxcBackend;
use crate::backend::{Device, InstanceConfig};
use crate::command::{CommandError, CommandExt};
use crate::mount::AccessMode;

impl crate::backend::InstanceBackend for LxcBackend {
    type Error = CommandError;

    fn create(&self, name: &str, config: &InstanceConfig) -> Result<(), Self::Error> {
        self.ensure_project()?;

        let mut cmd = self.lxc_project_command();
        cmd.args(["init", &config.image, name]);

        // Root disk -- profile provides pool and path; we override size
        // when a non-default disk size is configured.
        if let Some(ref size) = config.disk_size {
            cmd.args(["--device", &format!("root,size={size}")]);
        }
        if config.ephemeral {
            cmd.arg("--ephemeral");
        }
        if config.vm {
            cmd.arg("--vm");
            if !config.secure_boot {
                cmd.args(["--config", "boot.mode=uefi-nosecureboot"]);
            }
        }
        if let Some(n) = config.cpu {
            cmd.args(["--config", &format!("limits.cpu={n}")]);
        }
        if let Some(ref m) = config.memory {
            cmd.args(["--config", &format!("limits.memory={m}")]);
        }
        if let Some(p) = config.processes {
            if config.vm {
                tracing::warn!("limits.processes is ignored on VMs");
            } else {
                cmd.args(["--config", &format!("limits.processes={p}")]);
            }
        }
        if let Some(prio) = config.disk_priority {
            if config.vm {
                tracing::warn!("limits.disk.priority is ignored on VMs");
            } else {
                cmd.args(["--config", &format!("limits.disk.priority={prio}")]);
            }
        }
        if let Some(ref mode) = config.memory_enforce {
            if config.vm {
                tracing::warn!("limits.memory.enforce is ignored on VMs");
            } else {
                cmd.args(["--config", &format!("limits.memory.enforce={mode}")]);
            }
        }
        if let Some(ref idmap) = config.raw_idmap {
            cmd.args(["--config", &format!("raw.idmap={idmap}")]);
        }
        if config.security_nesting {
            if config.vm {
                tracing::warn!("security.nesting is ignored on VMs");
            } else {
                cmd.args(["--config", "security.nesting=true"]);
            }
        }

        cmd.args(["--config", "security.devlxd=false"]);
        cmd.run()?;

        Ok(())
    }

    fn start(&self, name: &str) -> Result<(), Self::Error> {
        self.lxc_project_command().args(["start", name]).run()
    }

    fn delete(&self, name: &str) -> Result<(), Self::Error> {
        self.lxc_project_command()
            .args(["delete", "--force", name])
            .run()
    }

    fn add_device(&self, name: &str, dev_name: &str, device: &Device) -> Result<(), Self::Error> {
        match device {
            Device::Disk {
                source,
                target,
                access,
            } => {
                let source_arg = format!("source={}", source.display());
                let path_arg = format!("path={}", target.display());
                let mut args = vec![
                    "config",
                    "device",
                    "add",
                    name,
                    dev_name,
                    "disk",
                    &source_arg,
                    &path_arg,
                ];
                if access == &AccessMode::ReadOnly {
                    args.push("readonly=true");
                }
                self.lxc_project_command().args(&args).run()
            },
            Device::Proxy {
                connect,
                listen,
                uid,
                gid,
                host_uid,
                host_gid,
                bind,
                mode,
            } => self
                .lxc_project_command()
                .args([
                    "config",
                    "device",
                    "add",
                    name,
                    dev_name,
                    "proxy",
                    &format!("bind={bind}"),
                    &format!("connect={connect}"),
                    &format!("listen={listen}"),
                    &format!("mode={mode}"),
                    &format!("uid={uid}"),
                    &format!("gid={gid}"),
                    &format!("security.uid={host_uid}"),
                    &format!("security.gid={host_gid}"),
                ])
                .run(),
        }
    }

    fn attach_to_bridge(
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
        self.lxc_project_command().args(&args).run()
    }

    fn set_description(&self, name: &str, desc: &str) -> Result<(), Self::Error> {
        self.lxc_project_command()
            .args([
                "config",
                "set",
                name,
                "--property",
                &format!("description={desc}"),
            ])
            .run()
    }

    fn exec(
        &self,
        name: &str,
        command: &[String],
        env: &HashMap<String, String>,
        cwd: &Path,
        uid: u32,
        gid: u32,
        home: Option<&Path>,
        proxy_url: Option<&str>,
    ) -> Result<i32, Self::Error> {
        let mut cmd = self.lxc_project_command();
        cmd.arg("exec")
            .arg(name)
            .arg("--user")
            .arg(uid.to_string())
            .arg("--group")
            .arg(gid.to_string())
            .arg("--cwd")
            .arg(cwd);

        for (k, v) in env {
            cmd.arg("--env").arg(format!("{k}={v}"));
        }
        if let Some(h) = home {
            let mut home_env = std::ffi::OsString::from("HOME=");
            home_env.push(h);
            cmd.arg("--env").arg(home_env);
        }
        if let Some(proxy) = proxy_url {
            cmd.arg("--env").arg(format!("HTTP_PROXY={proxy}"));
            cmd.arg("--env").arg(format!("HTTPS_PROXY={proxy}"));
            cmd.arg("--env").arg("NO_PROXY=");
        }

        cmd.arg("--");
        cmd.args(command);

        let code = cmd.status()?.code().unwrap_or(1);
        Ok(code)
    }

    fn exec_stdout(&self, name: &str, command: &[&str]) -> Result<String, Self::Error> {
        let mut cmd = self.lxc_project_command();
        cmd.args(["exec", name, "--"]);
        cmd.args(command);
        cmd.run_stdout()
    }

    fn write_file(
        &self,
        name: &str,
        content: &[u8],
        path: &str,
        mode: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), Self::Error> {
        use std::io::Write as _;
        use std::process::Stdio;

        let dest = format!("{name}/{path}");
        let mut child = self
            .lxc_project_command()
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
            .stdin(Stdio::piped())
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
            .map_err(CommandError::Io)?;

        let status = child.wait().map_err(CommandError::Io)?;
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
