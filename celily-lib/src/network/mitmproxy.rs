use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixListener;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{env, fs};

use thiserror::Error;
use tracing::{debug, error, info, warn};

use super::params::{DNS_PORT, PROXY_PORT};
use super::rule::NetworkRule;
use crate::command::{ChildExt, ShutdownStatus};
use crate::secrets::{SecretError, SecretProvider};
use crate::{CleanupDir, CleanupPath};

// ---------------------------------------------------------------------------
// MitmProxyError
// ---------------------------------------------------------------------------

/// Errors that can occur when starting or running mitmdump.
#[derive(Debug, Error)]
pub enum MitmProxyError {
    /// An I/O error with context describing the operation that failed.
    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },

    /// A secret resolution error.
    #[error(transparent)]
    Secret(#[from] SecretError),

    /// JSON serialization or deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// A required environment variable is not set.
    #[error("environment variable {var} is not set")]
    MissingEnv { var: &'static str },

    /// mitmdump did not connect to the config socket in time.
    #[error(
        "timed out waiting for mitmdump to connect to config socket; check {}",
        log_path.display()
    )]
    Timeout { log_path: PathBuf },

    /// mitmdump's response did not contain a valid CA certificate.
    #[error("mitmdump did not return a CA certificate: {reason}")]
    CaCert { reason: &'static str },
}

impl MitmProxyError {
    fn io_ctx(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

// ---------------------------------------------------------------------------
// MitmProxy
// ---------------------------------------------------------------------------

/// Timeout for `accept()` and `recv()` on the config socket.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

/// RAII guard for a per-instance mitmdump process spawned directly.
///
/// Creates a confdir for mitmdump's certificate store, opens a log file
/// for stdout/stderr, binds a Unix socket for one-shot config delivery,
/// spawns mitmdump, sends config over the socket, and receives the
/// generated CA certificate.
///
/// The config socket is cleaned up immediately after the exchange
/// (via [`CleanupPath`]). The confdir is cleaned up on drop
/// (via [`CleanupDir`]).
/// The log file is kept on failure, removed on clean exit.
pub struct MitmProxy {
    child: std::process::Child,
    pub port: u16,
    log_path: PathBuf,
    _confdir: CleanupDir,
}

impl MitmProxy {
    /// Create the confdir, open the log file, bind the config socket,
    /// spawn mitmdump, deliver the allowlist over the socket, and receive
    /// the CA certificate.
    ///
    /// Returns the proxy guard and the PEM-encoded CA certificate.
    pub(crate) fn start(
        bridge_name: &str,
        gateway_ip: &str,
        dns_filter: bool,
        allow: &[NetworkRule],
        provider: Option<&dyn SecretProvider<Error = SecretError>>,
    ) -> Result<(Self, String), MitmProxyError> {
        let runtime_dir =
            PathBuf::from(
                env::var("XDG_RUNTIME_DIR").map_err(|_| MitmProxyError::MissingEnv {
                    var: "XDG_RUNTIME_DIR",
                })?,
            );
        let state_dir = PathBuf::from(env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_default();
            format!("{home}/.local/state")
        }));

        // --- Confdir (mitmdump's certificate store) ---
        let confdir = runtime_dir.join(format!("celily-mitmproxy-{bridge_name}"));
        fs::create_dir_all(&confdir).map_err(|e| {
            MitmProxyError::io_ctx(format!("failed to create confdir {}", confdir.display()), e)
        })?;
        let confdir_cleanup = CleanupDir::new(confdir.clone());

        // --- Log file ---
        let log_dir = state_dir.join("celily/logs");
        fs::create_dir_all(&log_dir).map_err(|e| {
            MitmProxyError::io_ctx(format!("failed to create log dir {}", log_dir.display()), e)
        })?;
        let log_path = log_dir.join(format!("mitmdump_{bridge_name}.log"));
        let log_file = fs::File::create(&log_path).map_err(|e| {
            MitmProxyError::io_ctx(
                format!("failed to create log file {}", log_path.display()),
                e,
            )
        })?;

        // --- Config socket ---
        let socket_path = runtime_dir.join(format!("celily/mitmproxy-{bridge_name}.sock"));
        fs::create_dir_all(socket_path.parent().unwrap())
            .map_err(|e| MitmProxyError::io_ctx("failed to create celily runtime dir", e))?;

        // --- Resolve auth secrets (each unique name once) ---
        let auth_secrets = Self::resolve_auth_secrets(allow, provider)?;

        // --- Build config JSON ---
        let config_json = Self::build_config_json(dns_filter, allow, &auth_secrets)?;

        // mitmdump connects to this socket during startup, so the
        // socket must be listening before we spawn the process.
        let (socket_cleanup, listener) = Self::bind_config_socket(&socket_path)?;

        info!(
            gateway = %gateway_ip,
            port = PROXY_PORT,
            confdir = %confdir.display(),
            log = %log_path.display(),
            socket = %socket_path.display(),
            "spawning mitmdump"
        );

        let child = Command::new("mitmdump")
            .args([
                "--mode",
                &format!("regular@{gateway_ip}:{PROXY_PORT}"),
                "--mode",
                &format!("dns@{gateway_ip}:{DNS_PORT}"),
                "--set",
                &format!("confdir={}", confdir.display()),
                "--scripts",
                "/usr/share/celily/celily-mitmproxy.py",
                "--flow-detail",
                "2",
            ])
            .env("CELILY_CONFIG_SOCKET", &socket_path)
            .stdout(
                log_file
                    .try_clone()
                    .map_err(|e| MitmProxyError::io_ctx("failed to clone log file handle", e))?,
            )
            .stderr(log_file)
            .spawn()
            .map_err(|e| MitmProxyError::io_ctx("failed to spawn mitmdump", e))?;

        // --- Exchange config for CA cert ---
        let ca_cert = Self::exchange_config(listener, &config_json, &log_path)?;
        drop(socket_cleanup);

        info!(
            log = %log_path.display(),
            "received CA cert from mitmdump"
        );

        Ok((
            Self {
                child,
                port: PROXY_PORT,
                log_path: log_path.clone(),
                _confdir: confdir_cleanup,
            },
            ca_cert,
        ))
    }

    /// Resolve each unique `auth.secret` in the allow rules via the
    /// configured provider, or error if secrets are present but no provider
    /// is configured.
    fn resolve_auth_secrets(
        allow: &[NetworkRule],
        provider: Option<&dyn SecretProvider<Error = SecretError>>,
    ) -> Result<HashMap<String, String>, MitmProxyError> {
        let mut secrets: HashMap<String, String> = HashMap::new();
        for rule in allow {
            if let NetworkRule::Http {
                auth: Some(auth), ..
            } = rule
                && !secrets.contains_key(&auth.secret)
            {
                let p = provider.ok_or_else(|| SecretError::NoProvider {
                    secret: auth.secret.clone(),
                })?;
                let value = p.resolve(&auth.secret)?;
                secrets.insert(auth.secret.clone(), value);
            }
        }
        Ok(secrets)
    }

    /// Build the config JSON blob to send to mitmdump.
    fn build_config_json(
        dns_filter: bool,
        allow: &[NetworkRule],
        auth_secrets: &HashMap<String, String>,
    ) -> Result<String, MitmProxyError> {
        let json_allow: Vec<serde_json::Value> = allow
            .iter()
            .filter_map(|rule| match rule {
                NetworkRule::Http {
                    host,
                    path_prefixes,
                    auth,
                    headers,
                    methods,
                    quota,
                } => {
                    let mut obj = serde_json::json!({"host": host.to_lowercase()});
                    if let Some(prefixes) = path_prefixes {
                        if !prefixes.is_empty() {
                            obj["path_prefixes"] = serde_json::json!(prefixes);
                        }
                    }
                    if !headers.is_empty() {
                        obj["headers"] = serde_json::json!(headers);
                    }
                    if let Some(auth_cfg) = auth {
                        let resolved = &auth_secrets[&auth_cfg.secret];
                        let value = match &auth_cfg.prefix {
                            Some(prefix) => format!("{prefix}{resolved}"),
                            None => resolved.clone(),
                        };
                        obj["auth"] = serde_json::json!({
                            "header": &auth_cfg.header,
                            "value": value,
                        });
                    }
                    if let Some(methods_list) = methods {
                        obj["methods"] = serde_json::json!(methods_list);
                    } else {
                        obj["methods"] = serde_json::json!(["GET", "HEAD"]);
                    }
                    if let Some(q) = quota {
                        let window_seconds = q.window.as_secs();
                        obj["quota"] = serde_json::json!({
                            "max_requests": q.max_requests,
                            "window_seconds": window_seconds,
                        });
                    }
                    Some(obj)
                },
                NetworkRule::Tcp { .. } => None,
            })
            .collect();

        Ok(serde_json::to_string(&serde_json::json!({
            "allow": json_allow,
            "dns": dns_filter,
        }))?)
    }

    /// Create the Unix domain socket and start listening.
    ///
    /// Returns a [`CleanupPath`] guard and the bound listener.
    /// The caller must spawn mitmdump after this returns
    /// (so mitmdump can `connect()`).
    fn bind_config_socket(
        socket_path: &Path,
    ) -> Result<(CleanupPath, UnixListener), MitmProxyError> {
        // Remove leftover from a previous crash. Only ignore NotFound
        // other errors (e.g. ENOPERM) indicate something wrong.
        if let Err(e) = fs::remove_file(socket_path)
            && e.kind() != io::ErrorKind::NotFound
        {
            warn!(
                path = %socket_path.display(),
                error = %e,
                "failed to remove leftover socket file"
            );
        }

        let cleanup = CleanupPath::new(socket_path.to_path_buf());
        let listener = UnixListener::bind(cleanup.as_path()).map_err(|e| {
            MitmProxyError::io_ctx(
                format!("failed to bind Unix socket at {}", socket_path.display()),
                e,
            )
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|e| MitmProxyError::io_ctx("failed to set socket nonblocking", e))?;

        Ok((cleanup, listener))
    }

    /// Accept a connection from mitmdump, send the config JSON, and
    /// receive the CA certificate.
    fn exchange_config(
        listener: UnixListener,
        config_json: &str,
        log_path: &Path,
    ) -> Result<String, MitmProxyError> {
        // --- Accept connection (mitmdump connected via CELILY_CONFIG_SOCKET) ---
        let deadline = Instant::now() + SOCKET_TIMEOUT;
        let (mut stream, _addr) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        return Err(MitmProxyError::Timeout {
                            log_path: log_path.to_path_buf(),
                        });
                    }
                    sleep(Duration::from_millis(50));
                },
                Err(e) => {
                    return Err(MitmProxyError::io_ctx("accept on config socket failed", e));
                },
            }
        };
        drop(listener);

        // --- Send config JSON ---
        stream
            .write_all(config_json.as_bytes())
            .map_err(|e| MitmProxyError::io_ctx("failed to send config to mitmdump", e))?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|e| MitmProxyError::io_ctx("failed to shutdown socket write", e))?;

        // --- Receive CA cert ---
        stream
            .set_read_timeout(Some(SOCKET_TIMEOUT))
            .map_err(|e| MitmProxyError::io_ctx("failed to set socket read timeout", e))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|e| MitmProxyError::io_ctx("failed to read CA cert from mitmdump", e))?;

        if response.is_empty() {
            return Err(MitmProxyError::CaCert {
                reason: "empty response from mitmdump",
            });
        }

        let ca_cert: String = serde_json::from_slice(&response)
            .map(|v: serde_json::Value| v["ca_cert"].as_str().unwrap_or("").to_string())?;

        if ca_cert.is_empty() {
            return Err(MitmProxyError::CaCert {
                reason: "response missing 'ca_cert' field",
            });
        }

        Ok(ca_cert)
    }
}

impl Drop for MitmProxy {
    fn drop(&mut self) {
        info!(log = %self.log_path.display(), "stopping mitmdump");

        match self.child.shutdown(Duration::from_secs(2)) {
            ShutdownStatus::AlreadyExited(status) => {
                error!(
                    log = %self.log_path.display(),
                    code = ?status.code(),
                    "mitmdump exited unexpectedly before shutdown; log preserved"
                );
            },
            ShutdownStatus::Exited(s) if s.success() => {
                debug!("mitmdump exited successfully");
                if let Err(e) = fs::remove_file(&self.log_path) {
                    debug!(log = %self.log_path.display(), error = %e, "failed to remove log file");
                }
            },
            // Signal termination from our SIGTERM or SIGKILL is expected.
            ShutdownStatus::Exited(s) if s.signal().is_some() => {
                debug!("mitmdump terminated by signal");
                if let Err(e) = fs::remove_file(&self.log_path) {
                    debug!(log = %self.log_path.display(), error = %e, "failed to remove log file");
                }
            },
            ShutdownStatus::Exited(s) => {
                error!(
                    log = %self.log_path.display(),
                    code = ?s.code(),
                    "mitmdump exited with error; log preserved"
                );
            },
            ShutdownStatus::WaitFailure(e) => {
                error!(
                    log = %self.log_path.display(),
                    error = %e,
                    "failed to wait on mitmdump; log preserved"
                );
            },
        }
    }
}
