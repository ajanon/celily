//! HTTP allowlist handler for the in-process proxelar proxy.
//!
//! Requests are matched against the HTTP allow rules in order; the first
//! match wins:
//!
//! - host: case-insensitive, against the request authority. proxelar connects
//!   upstream to that same authority: the host checked is the host reached.
//! - path prefixes: the request path starts with one of them, on a segment
//!   boundary (`/v2` covers `/v2` and `/v2/x`, not `/v20`), and has no segment
//!   a server could read as `.` or `..` (`%2e%2e`, `..%2f`, `..;`): those could
//!   resolve outside the prefix.
//! - methods: case-insensitive, `GET` and `HEAD` when the rule has none.
//!
//! A matching request is counted against the rule's quota (429 once
//! exhausted), gets the rule's static headers and then its auth header,
//! and is forwarded. Anything else gets a 403.
//!
//! Raw tunnels (CONNECT payloads that are neither TLS nor HTTP) are always
//! closed: HTTP rules only allow HTTP requests, and TCP rules are enforced
//! by the bridge ACL, without the proxy.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use http::uri::Authority;
use http::{HeaderMap, StatusCode};
use proxyapi::{
    HttpContext,
    HttpHandler,
    ProxyRequest,
    ProxyResponse,
    RequestOrResponse,
    TunnelDecision,
};
use tracing::warn;

use super::rule::{NetworkRule, QuotaConfig};

/// Methods allowed by a rule that does not list any.
const DEFAULT_METHODS: [&str; 2] = ["GET", "HEAD"];

/// HTTP handler enforcing celily's allowlist.
///
/// `Clone` is cheap (refcount bumps): the proxy clones the handler for
/// every connection, and all clones share the quota state.
#[derive(Clone)]
pub(crate) struct CelilyHandler {
    rules: Arc<[NetworkRule]>,
    /// Resolved auth secrets: secret name -> value.
    auth_secrets: Arc<HashMap<String, String>>,
    /// Quota state, one entry per rule (same index as `rules`).
    quotas: Arc<[Mutex<QuotaWindow>]>,
}

impl CelilyHandler {
    pub(crate) fn new(rules: Vec<NetworkRule>, auth_secrets: HashMap<String, String>) -> Self {
        let quotas = rules.iter().map(|_| Mutex::default()).collect();
        Self {
            rules: rules.into(),
            auth_secrets: Arc::new(auth_secrets),
            quotas,
        }
    }

    fn reject(&mut self, status: StatusCode, body: &'static [u8]) -> RequestOrResponse {
        RequestOrResponse::Response(self.synthetic_protocol_response(
            status,
            HeaderMap::new(),
            Bytes::from_static(body),
        ))
    }
}

#[async_trait]
impl HttpHandler for CelilyHandler {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        mut request: ProxyRequest,
    ) -> RequestOrResponse {
        let Some(host) = request.head.uri.host() else {
            return self.reject(StatusCode::BAD_REQUEST, b"request is missing a host\n");
        };
        let path = request.head.uri.path();
        let method = request.head.method.as_str();
        let Some(index) = self
            .rules
            .iter()
            .position(|rule| rule_matches(rule, host, path, method))
        else {
            return self.reject(StatusCode::FORBIDDEN, b"Blocked by network policy\n");
        };

        let rules = Arc::clone(&self.rules);
        let NetworkRule::Http {
            auth,
            headers,
            quota,
            ..
        } = &rules[index]
        else {
            unreachable!("rule_matches only matches HTTP rules");
        };

        if let Some(quota) = quota {
            let allowed = self.quotas[index]
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .try_take(quota, unix_now_secs());
            if !allowed {
                return self.reject(StatusCode::TOO_MANY_REQUESTS, b"Request quota exceeded\n");
            }
        }

        // Static headers first, then auth, so auth wins on a name clash.
        for (name, value) in headers {
            if let Err(e) = request.head.headers.set(name, value) {
                warn!(header = %name, error = %e, "not injecting invalid static header");
            }
        }
        if let Some(auth) = auth
            && let Some(secret) = self.auth_secrets.get(&auth.secret)
        {
            let value = format!("{}{secret}", auth.prefix.as_deref().unwrap_or_default());
            if let Err(e) = request.head.headers.set(&auth.header, value) {
                warn!(header = %auth.header, error = %e, "not injecting invalid auth header");
            }
        }

        RequestOrResponse::Request(request)
    }

    async fn handle_response(
        &mut self,
        _ctx: &HttpContext,
        response: ProxyResponse,
    ) -> ProxyResponse {
        response
    }

    async fn handle_tunnel(
        &mut self,
        _ctx: &HttpContext,
        _authority: &Authority,
    ) -> TunnelDecision {
        TunnelDecision::Close
    }
}

/// Whether `rule` is an HTTP rule allowing this request.
fn rule_matches(rule: &NetworkRule, host: &str, path: &str, method: &str) -> bool {
    let NetworkRule::Http {
        host: rule_host,
        path_prefixes,
        methods,
        ..
    } = rule
    else {
        return false;
    };
    let path_allowed = match path_prefixes.as_deref() {
        None | Some([]) => true,
        Some(prefixes) => {
            !may_escape_prefix(path) && prefixes.iter().any(|p| path_has_prefix(path, p))
        },
    };
    let method_allowed = match methods {
        Some(methods) => methods.iter().any(|m| m.eq_ignore_ascii_case(method)),
        None => DEFAULT_METHODS
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method)),
    };
    host.eq_ignore_ascii_case(rule_host) && path_allowed && method_allowed
}

/// Whether `path` is `prefix` or below it: the prefix must end on a
/// segment boundary, so `/v2` covers `/v2` and `/v2/x` but not `/v20`.
fn path_has_prefix(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || prefix.ends_with('/'))
}

/// Whether a server could resolve `path` outside a prefix it starts with:
/// a segment reads as `.` or `..` once percent-decoded, split on `\` as
/// well as `/`, and cut at `;` (`..;` is `..` to servers that drop path
/// parameters).
///
/// Ceiling: servers folding non-ASCII into dots (U+FF0E, overlong UTF-8
/// `%c0%ae`) are not covered. Refusing non-ASCII bytes would cover them.
fn may_escape_prefix(path: &str) -> bool {
    percent_decode(path)
        .split(|&b| b == b'/' || b == b'\\')
        .map(|segment| segment.split(|&b| b == b';').next().unwrap_or_default())
        .any(|segment| segment == b"." || segment == b"..")
}

/// `path` with every valid `%XX` escape decoded, once. Invalid escapes are
/// kept as they are.
fn percent_decode(path: &str) -> Vec<u8> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(&[hi, lo]) = bytes.get(i + 1..i + 3)
            && let (Some(hi), Some(lo)) = (hex_digit(hi), hex_digit(lo))
        {
            decoded.push((hi << 4) | lo);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    decoded
}

fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

/// Fixed-window request counter of one rule. Windows are aligned to the
/// wall clock: the count resets when a new window starts.
#[derive(Default)]
struct QuotaWindow {
    /// Index of the current window: Unix seconds divided by the window length.
    index: u64,
    /// Requests counted in the current window.
    count: u64,
}

impl QuotaWindow {
    /// Count one request at `now` (Unix seconds). Returns false, without
    /// counting, when the current window is already full.
    fn try_take(&mut self, quota: &QuotaConfig, now: u64) -> bool {
        let index = now / quota.window.as_secs().max(1);
        if index != self.index {
            self.index = index;
            self.count = 0;
        }
        if self.count >= quota.max_requests {
            return false;
        }
        self.count += 1;
        true
    }
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use http::Method;
    use proxyapi::{ProxyBody, RequestHead};

    use super::*;
    use crate::network::rule::AuthConfig;

    fn request(method: &str, uri: &str) -> ProxyRequest {
        let head = RequestHead::new(
            Method::from_bytes(method.as_bytes()).unwrap(),
            uri.parse().unwrap(),
            http::Version::HTTP_11,
            proxyapi::header::from_http(&HeaderMap::new()),
        );
        ProxyRequest::new(head, ProxyBody::empty())
    }

    fn http_rule(host: &str) -> NetworkRule {
        NetworkRule::Http {
            host: host.into(),
            path_prefixes: None,
            auth: None,
            headers: HashMap::new(),
            methods: None,
            quota: None,
        }
    }

    fn hourly(max_requests: u64) -> QuotaConfig {
        QuotaConfig {
            max_requests,
            window: Duration::from_secs(3600),
        }
    }

    async fn handle(handler: &mut CelilyHandler, method: &str, uri: &str) -> RequestOrResponse {
        let ctx = HttpContext {
            remote_addr: "127.0.0.1:0".parse().unwrap(),
        };
        handler.handle_request(&ctx, request(method, uri)).await
    }

    /// Status of the synthesized response, or `None` when forwarded.
    async fn status(handler: &mut CelilyHandler, method: &str, uri: &str) -> Option<StatusCode> {
        match handle(handler, method, uri).await {
            RequestOrResponse::Request(_) => None,
            RequestOrResponse::Response(r) => Some(r.head.status),
        }
    }

    async fn forwarded(handler: &mut CelilyHandler, method: &str, uri: &str) -> bool {
        status(handler, method, uri).await.is_none()
    }

    #[tokio::test]
    async fn allowed_host_is_forwarded() {
        let mut h = CelilyHandler::new(vec![http_rule("api.example.com")], HashMap::new());
        assert!(forwarded(&mut h, "GET", "http://api.example.com/").await);
        assert!(forwarded(&mut h, "GET", "https://API.Example.com/x").await);
    }

    #[tokio::test]
    async fn other_hosts_get_403() {
        let mut h = CelilyHandler::new(vec![http_rule("api.example.com")], HashMap::new());
        let s = status(&mut h, "GET", "http://evil.com/").await;
        assert_eq!(s, Some(StatusCode::FORBIDDEN));
        let s = status(&mut h, "GET", "http://sub.api.example.com/").await;
        assert_eq!(s, Some(StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn missing_host_gets_400() {
        let mut h = CelilyHandler::new(vec![http_rule("api.example.com")], HashMap::new());
        let s = status(&mut h, "GET", "/").await;
        assert_eq!(s, Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn methods_default_to_get_and_head() {
        let mut h = CelilyHandler::new(vec![http_rule("api.example.com")], HashMap::new());
        assert!(forwarded(&mut h, "HEAD", "http://api.example.com/").await);
        let s = status(&mut h, "POST", "http://api.example.com/").await;
        assert_eq!(s, Some(StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn listed_methods_replace_the_default() {
        let rule = NetworkRule::Http {
            host: "api.example.com".into(),
            path_prefixes: None,
            auth: None,
            headers: HashMap::new(),
            methods: Some(vec!["post".into()]),
            quota: None,
        };
        let mut h = CelilyHandler::new(vec![rule], HashMap::new());
        assert!(forwarded(&mut h, "POST", "http://api.example.com/").await);
        let s = status(&mut h, "GET", "http://api.example.com/").await;
        assert_eq!(s, Some(StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn path_prefixes_restrict_the_rule() {
        let rule = NetworkRule::Http {
            host: "api.example.com".into(),
            path_prefixes: Some(vec!["/v1/".into(), "/v3/".into()]),
            auth: None,
            headers: HashMap::new(),
            methods: None,
            quota: None,
        };
        let mut h = CelilyHandler::new(vec![rule], HashMap::new());
        assert!(forwarded(&mut h, "GET", "http://api.example.com/v3/x").await);
        let s = status(&mut h, "GET", "http://api.example.com/v2/x").await;
        assert_eq!(s, Some(StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn first_matching_rule_wins() {
        let narrow = NetworkRule::Http {
            host: "api.example.com".into(),
            path_prefixes: Some(vec!["/v1/".into()]),
            auth: None,
            headers: HashMap::new(),
            methods: None,
            quota: Some(hourly(1)),
        };
        let mut h = CelilyHandler::new(vec![narrow, http_rule("api.example.com")], HashMap::new());
        assert!(forwarded(&mut h, "GET", "http://api.example.com/v1/x").await);
        // The narrow rule matches first: its quota applies, no fallthrough.
        let s = status(&mut h, "GET", "http://api.example.com/v1/x").await;
        assert_eq!(s, Some(StatusCode::TOO_MANY_REQUESTS));
        // Outside the narrow rule, the broad one matches.
        assert!(forwarded(&mut h, "GET", "http://api.example.com/v2/x").await);
    }

    #[tokio::test]
    async fn quota_is_shared_between_clones() {
        let rule = NetworkRule::Http {
            host: "api.example.com".into(),
            path_prefixes: None,
            auth: None,
            headers: HashMap::new(),
            methods: None,
            quota: Some(hourly(2)),
        };
        let mut h = CelilyHandler::new(vec![rule], HashMap::new());
        let mut clone = h.clone();
        assert!(forwarded(&mut h, "GET", "http://api.example.com/").await);
        assert!(forwarded(&mut clone, "GET", "http://api.example.com/").await);
        let s = status(&mut h, "GET", "http://api.example.com/").await;
        assert_eq!(s, Some(StatusCode::TOO_MANY_REQUESTS));
    }

    #[test]
    fn path_prefixes_end_on_a_segment_boundary() {
        assert!(path_has_prefix("/v2", "/v2"));
        assert!(path_has_prefix("/v2/x", "/v2"));
        assert!(path_has_prefix("/v2/x", "/v2/"));
        assert!(path_has_prefix("/anything", "/"));
        assert!(!path_has_prefix("/v20", "/v2"));
        assert!(!path_has_prefix("/v2", "/v2/"));
        assert!(!path_has_prefix(
            "/repos/canonicalevil/x",
            "/repos/canonical"
        ));
    }

    #[tokio::test]
    async fn dot_segments_never_match_a_path_prefix() {
        let rule = NetworkRule::Http {
            host: "api.example.com".into(),
            path_prefixes: Some(vec!["/v1/".into()]),
            auth: None,
            headers: HashMap::new(),
            methods: None,
            quota: None,
        };
        let mut h = CelilyHandler::new(vec![rule], HashMap::new());
        assert!(forwarded(&mut h, "GET", "http://api.example.com/v1/x").await);
        let s = status(&mut h, "GET", "http://api.example.com/v1/%2e%2e/v2/x").await;
        assert_eq!(s, Some(StatusCode::FORBIDDEN));
    }

    #[test]
    fn dot_segments_may_escape_a_prefix() {
        for path in [
            "/v1/../v2",
            "/v1/x/..",
            "/v1/./x",
            "/v1/%2e%2e/v2",
            "/v1/.%2E/v2",
            "/v1/..%2fv2",
            "/v1/..%5Cv2",
            "/v1\\..\\v2",
            "/v1/..;x/v2",
        ] {
            assert!(may_escape_prefix(path), "{path}");
        }
        for path in ["/v1/x", "/v1/...", "/v1/.x", "/v1/x..", "/v1/a%2Fb"] {
            assert!(!may_escape_prefix(path), "{path}");
        }
    }

    #[test]
    fn percent_decode_decodes_valid_escapes_once() {
        assert_eq!(percent_decode("/a%2Fb%2e"), b"/a/b.");
        assert_eq!(percent_decode("%252e"), b"%2e");
        assert_eq!(percent_decode("%zz%2%"), b"%zz%2%");
        assert_eq!(percent_decode("%%2e"), b"%.");
    }

    #[test]
    fn quota_window_resets_when_a_new_window_starts() {
        let quota = QuotaConfig {
            max_requests: 1,
            window: Duration::from_secs(60),
        };
        let mut w = QuotaWindow::default();
        assert!(w.try_take(&quota, 120));
        assert!(!w.try_take(&quota, 179));
        assert!(w.try_take(&quota, 180));
    }

    #[tokio::test]
    async fn headers_then_auth_are_injected() {
        let rule = NetworkRule::Http {
            host: "api.example.com".into(),
            path_prefixes: None,
            auth: Some(AuthConfig {
                secret: "token".into(),
                header: "Authorization".into(),
                prefix: Some("Bearer ".into()),
            }),
            headers: HashMap::from([
                ("x-static".into(), "static".into()),
                ("authorization".into(), "overridden".into()),
            ]),
            methods: None,
            quota: None,
        };
        let secrets = HashMap::from([("token".into(), "s3cret".into())]);
        let mut h = CelilyHandler::new(vec![rule], secrets);
        let RequestOrResponse::Request(r) = handle(&mut h, "GET", "http://api.example.com/").await
        else {
            panic!("expected the request to be forwarded");
        };
        assert_eq!(r.head.headers.get("x-static"), Some(b"static".as_slice()));
        assert_eq!(
            r.head.headers.get("authorization"),
            Some(b"Bearer s3cret".as_slice())
        );
    }

    #[tokio::test]
    async fn raw_tunnels_are_closed_even_to_allowed_hosts() {
        let mut h = CelilyHandler::new(vec![http_rule("api.example.com")], HashMap::new());
        let ctx = HttpContext {
            remote_addr: "127.0.0.1:0".parse().unwrap(),
        };
        let authority = Authority::from_static("api.example.com:22");
        let decision = h.handle_tunnel(&ctx, &authority).await;
        assert_eq!(decision, TunnelDecision::Close);
    }
}
