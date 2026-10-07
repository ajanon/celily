use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use async_trait::async_trait;

use super::LxcBackend;
use crate::backend::{Device, InstanceConfig};
use crate::command::{AsyncCommandExt, CommandError, CommandExt};
use crate::mount::AccessMode;

#[async_trait]
impl crate::backend::InstanceBackend for LxcBackend {
    type Error = CommandError;

    async fn create(&self, name: &str, config: &InstanceConfig) -> Result<(), Self::Error> {
        self.ensure_project().await?;

        let mut cmd = self.lxc_project_command_async();
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
        cmd.run().await?;

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

    async fn add_device(
        &self,
        name: &str,
        dev_name: &str,
        device: &Device,
    ) -> Result<(), Self::Error> {
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
                self.lxc_project_command_async().args(&args).run().await
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
            } => {
                self.lxc_project_command_async()
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
                    .run()
                    .await
            },
        }
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

    async fn set_description(&self, name: &str, desc: &str) -> Result<(), Self::Error> {
        self.lxc_project_command_async()
            .args([
                "config",
                "set",
                name,
                "--property",
                &format!("description={desc}"),
            ])
            .run()
            .await
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
}
