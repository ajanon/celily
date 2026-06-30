use std::collections::HashMap;
use std::time::Duration;

/// A request quota tied to an HTTP allow rule.
///
/// When present on an allow rule, requests matching that rule are
/// counted against the quota. Quotas use fixed time buckets aligned
/// to wall clock -- the counter resets when the bucket rolls over.
#[derive(Debug, Clone)]
pub struct QuotaConfig {
    /// Maximum number of HTTP requests allowed in the time window.
    pub max_requests: u64,

    /// Time window duration. Must be at least one second.
    pub window: Duration,
}

/// Authentication configuration for header injection on allowed hosts.
///
/// Secrets are resolved from `rbw` at startup and routed to mitmdump
/// only - they never appear as environment variables in the container.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
pub struct AuthConfig {
    /// Name of an `rbw` item to resolve.
    pub secret: String,

    /// HTTP header name to inject (e.g. `Authorization`, `x-api-key`).
    pub header: String,

    /// Optional prefix prepended to the resolved secret value.
    /// For `Authorization: Bearer <key>`, set `prefix = "Bearer "`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub prefix: Option<String>,
}

/// A single network isolation rule.
///
/// Rules are checked in config-file order; first match wins. Anything
/// not matched by a rule is blocked (default-deny).
///
/// TCP rules are enforced at the bridge ACL level (nftables) and
/// bypass the HTTP proxy entirely.
#[derive(Debug, Clone)]
pub enum NetworkRule {
    /// An HTTP/HTTPS host with optional path prefix, method
    /// restriction, auth injection, and static headers.
    /// All traffic to this host goes through the MITM proxy.
    Http {
        host: String,
        path_prefixes: Option<Vec<String>>,
        auth: Option<AuthConfig>,
        /// Static HTTP headers to inject on matching requests.
        /// Plain key-value pairs (no secret resolution).
        headers: HashMap<String, String>,
        /// Allowed HTTP methods (e.g. `["GET", "HEAD"]`).
        /// When absent, all methods are allowed.
        /// When present, only listed methods match the rule.
        methods: Option<Vec<String>>,
        /// Optional request quota tied to this allow rule.
        /// When present, requests matching this rule are counted
        /// against the quota. Exceeded quotas return HTTP 429.
        quota: Option<QuotaConfig>,
    },

    /// A raw TCP destination (IP address and ports).
    ///
    /// TCP rules are enforced by the bridge egress ACL -- traffic
    /// bypasses the HTTP proxy entirely. The `host` field must be an
    /// IP address; hostnames are not resolved. Ports are joined with
    /// commas into a single `destination_port` ACL entry.
    ///
    /// Known limitations: IP addresses only (no DNS resolution),
    /// no port-range syntax.
    Tcp {
        /// Destination IP address (IPv4 or IPv6).
        host: std::net::IpAddr,

        /// Destination TCP ports. Must be non-empty. Each value is a
        /// single port number; ranges are not yet supported.
        ports: Vec<u16>,
    },
}
