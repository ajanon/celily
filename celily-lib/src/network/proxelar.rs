//! In-process proxelar proxies: the HTTP forward proxy and the DNS proxy
//! of one instance.
//!
//! Both run as tasks on the caller's tokio runtime, bound to the bridge
//! gateway. The CA is generated in a private directory under
//! `$XDG_RUNTIME_DIR` before the proxies start: its certificate goes into
//! the instance, its key stays on the host. [`ProxelarGuard`] stops both
//! proxies and removes the CA directory on drop.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs, future, io};

use proxyapi::ca::Ssl;
use proxyapi::{DnsConfig, Proxy, ProxyConfig, ProxyMode, UpstreamTlsConfig};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::info;

use super::dns_handler::CelilyDnsHandler;
use super::handler::CelilyHandler;
use super::params::{DNS_PORT, PROXY_PORT};
use super::rule::NetworkRule;
use crate::CleanupDir;
use crate::backend::BridgeSubnet;

/// How long the HTTP proxy gets to start accepting connections.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the host resolver is read from. Allowed DNS queries go there.
const RESOLV_CONF: &str = "/etc/resolv.conf";

/// Errors that can occur when starting the proxelar proxies.
#[derive(Debug, thiserror::Error)]
pub enum ProxelarError {
    /// An I/O error with context describing the operation that failed.
    #[error("{context}: {source}")]
    Io { context: String, source: io::Error },

    /// proxelar returned an error.
    #[error(transparent)]
    Proxy(#[from] proxyapi::Error),

    /// A spawned task panicked.
    #[error("proxelar task panicked: {0}")]
    TaskPanic(String),

    /// A proxy task failed during startup.
    #[error("the proxelar {proxy} proxy failed: {source}")]
    Failed {
        proxy: &'static str,
        source: proxyapi::Error,
    },

    /// A proxy task stopped during startup without an error.
    #[error("the proxelar {0} proxy stopped during startup")]
    Stopped(&'static str),

    /// A required environment variable is not set.
    #[error("environment variable {var} is not set")]
    MissingEnv { var: &'static str },

    /// The HTTP proxy did not accept connections within the timeout.
    #[error("the proxelar HTTP proxy did not start in time")]
    Timeout,

    /// The generated CA certificate is not UTF-8.
    #[error("the proxelar CA certificate is not UTF-8: {0}")]
    CaCertEncoding(#[from] std::string::FromUtf8Error),

    /// The host resolver configuration names no usable nameserver.
    #[error("no nameserver with an IP address in {RESOLV_CONF}")]
    NoNameserver,
}

impl ProxelarError {
    fn io_ctx(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

type ProxyTask = JoinHandle<Result<(), proxyapi::Error>>;

/// RAII guard for the in-process proxies of one instance.
///
/// Dropping it aborts both proxy tasks (their listeners close; open
/// connections end with the instance) and then removes the CA directory.
pub struct ProxelarGuard {
    /// Port of the HTTP proxy on the bridge gateway.
    pub port: u16,
    http: ProxyTask,
    dns: ProxyTask,
    _ca_dir: CleanupDir,
}

impl ProxelarGuard {
    /// Start the HTTP and DNS proxies on the gateway of `subnet`, enforcing
    /// `rules` for the clients of `subnet`.
    ///
    /// Returns once the HTTP proxy accepts connections, with the
    /// PEM-encoded CA certificate the instance must trust.
    pub(crate) async fn start(
        bridge_name: &str,
        subnet: BridgeSubnet,
        rules: Vec<NetworkRule>,
        auth_secrets: HashMap<String, String>,
    ) -> Result<(Self, String), ProxelarError> {
        // proxelar builds its TLS servers with the process-wide rustls
        // provider. rustls picks none by itself when both ring and
        // aws-lc-rs are compiled in, which they are here. An error means
        // one is installed already.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let gateway_ip = IpAddr::V4(subnet.gateway);
        let dns_upstream = SocketAddr::new(host_nameserver()?, 53);
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR").ok_or(ProxelarError::MissingEnv {
            var: "XDG_RUNTIME_DIR",
        })?;
        let ca_dir = PathBuf::from(runtime_dir).join(format!("celily-proxelar-{bridge_name}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&ca_dir)
            .map_err(|e| ProxelarError::io_ctx(format!("creating {}", ca_dir.display()), e))?;
        let ca_dir_guard = CleanupDir::new(ca_dir.clone());
        let ca_cert = generate_ca(&ca_dir).await?;

        info!(
            gateway = %gateway_ip,
            http_port = PROXY_PORT,
            dns_port = DNS_PORT,
            %dns_upstream,
            "starting the proxelar proxies"
        );

        let http_proxy = Proxy::new(proxy_config(
            SocketAddr::new(gateway_ip, PROXY_PORT),
            ProxyMode::Forward,
            &ca_dir,
        ));
        let http_handler = CelilyHandler::new(rules.clone(), auth_secrets, subnet);
        let http = tokio::spawn(async move {
            http_proxy
                .start_with_handler(http_handler, future::pending())
                .await
        });

        // DNS mode does not use the CA.
        let dns_proxy = Proxy::new(proxy_config(
            SocketAddr::new(gateway_ip, DNS_PORT),
            ProxyMode::Dns {
                config: DnsConfig::new(dns_upstream),
            },
            &ca_dir,
        ));
        let dns_handler = CelilyDnsHandler::new(&rules);
        let dns = tokio::spawn(async move {
            dns_proxy
                .start_with_dns_handler(dns_handler, future::pending())
                .await
        });

        let mut guard = Self {
            port: PROXY_PORT,
            http,
            dns,
            _ca_dir: ca_dir_guard,
        };
        guard
            .wait_until_ready(SocketAddr::new(gateway_ip, PROXY_PORT))
            .await?;
        Ok((guard, ca_cert))
    }

    /// Wait for the HTTP proxy to accept connections, failing early with
    /// the proxy's own error if either task stops.
    ///
    /// The DNS proxy binds its socket as its first step, while the HTTP
    /// proxy still loads the CA, so a DNS bind failure shows up here in
    /// practice. One that came after the last check would go unnoticed:
    /// queries would then fail, never leak.
    async fn wait_until_ready(&mut self, http_addr: SocketAddr) -> Result<(), ProxelarError> {
        let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
        loop {
            self.check_tasks().await?;
            match tokio::time::timeout_at(deadline, TcpStream::connect(http_addr)).await {
                Ok(Ok(_)) => {
                    self.check_tasks().await?;
                    info!("the proxelar HTTP proxy accepts connections");
                    return Ok(());
                },
                Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                },
                Ok(Err(e)) => {
                    return Err(ProxelarError::io_ctx("connecting to the HTTP proxy", e));
                },
                Err(_) => return Err(ProxelarError::Timeout),
            }
        }
    }

    /// Fail with the task's own error if a proxy task has stopped.
    async fn check_tasks(&mut self) -> Result<(), ProxelarError> {
        for (name, task) in [("HTTP", &mut self.http), ("DNS", &mut self.dns)] {
            if task.is_finished() {
                return match task.await {
                    Ok(Ok(())) => Err(ProxelarError::Stopped(name)),
                    Ok(Err(source)) => Err(ProxelarError::Failed {
                        proxy: name,
                        source,
                    }),
                    Err(e) => Err(ProxelarError::TaskPanic(e.to_string())),
                };
            }
        }
        Ok(())
    }
}

impl Drop for ProxelarGuard {
    fn drop(&mut self) {
        info!("stopping the proxelar proxies");
        self.http.abort();
        self.dns.abort();
    }
}

/// Generate the CA in `dir` and return its PEM certificate.
async fn generate_ca(dir: &Path) -> Result<String, ProxelarError> {
    let dir = dir.to_path_buf();
    let ssl = tokio::task::spawn_blocking(move || Ssl::load_or_generate(&dir))
        .await
        .map_err(|e| ProxelarError::TaskPanic(e.to_string()))??;
    Ok(String::from_utf8(ssl.ca_cert_pem().to_vec())?)
}

fn proxy_config(addr: SocketAddr, mode: ProxyMode, ca_dir: &Path) -> ProxyConfig {
    // Nothing reads the events: with the receiver dropped, every send
    // fails at once instead of filling the channel.
    let (event_tx, _) = mpsc::channel(1);
    ProxyConfig {
        addr,
        mode,
        event_tx,
        ca_dir: ca_dir.to_path_buf(),
        upstream_tls: UpstreamTlsConfig::Default,
        intercept: None,
        body_capture_limit: None,
        replay_rx: None,
    }
}

/// The first nameserver of the host resolver configuration.
fn host_nameserver() -> Result<IpAddr, ProxelarError> {
    let contents = fs::read_to_string(RESOLV_CONF)
        .map_err(|e| ProxelarError::io_ctx(format!("reading {RESOLV_CONF}"), e))?;
    first_nameserver(&contents).ok_or(ProxelarError::NoNameserver)
}

/// The first `nameserver` line of `resolv_conf` holding a plain IP
/// address. Scoped IPv6 addresses (`fe80::1%eth0`) are skipped.
fn first_nameserver(resolv_conf: &str) -> Option<IpAddr> {
    resolv_conf.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next() == Some("nameserver"))
            .then(|| fields.next()?.parse().ok())
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_nameserver_skips_comments_and_unusable_entries() {
        let conf = "# nameserver 192.0.2.1\nsearch example.org\nnameserver \
                    fe80::1%eth0\nnameserver\t127.0.0.53\nnameserver 192.0.2.2\n";
        assert_eq!(first_nameserver(conf), Some("127.0.0.53".parse().unwrap()));
        assert_eq!(first_nameserver("search example.org\n"), None);
    }
}
