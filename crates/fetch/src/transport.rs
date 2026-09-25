//! HTTP transport abstraction.
//!
//! The resume/verify logic is transport-independent and tested against a
//! real local server. The default transport is `ureq` with rustls and the
//! platform certificate verifier (so corporate TLS-inspection roots
//! installed in the OS store are honoured).

use std::fmt;
use std::io::Read;
use std::time::Duration;

/// A response to a (possibly ranged) GET.
pub struct Response {
    pub status: u16,
    /// Start offset of the body. `0` for a full (200) response.
    pub range_start: u64,
    /// Total size of the resource, if the server reported it.
    pub total_len: Option<u64>,
    /// `ETag` (preferred) or `Last-Modified`, used for `If-Range`.
    pub validator: Option<String>,
    pub body: Box<dyn Read + Send>,
}

#[derive(Debug)]
pub enum TransportError {
    /// DNS, connect, TLS handshake, reset.
    Connection(String),
    /// A timeout while waiting for headers or body.
    Timeout,
    /// The URL is not allowed (e.g. plain HTTP when HTTPS is required).
    Forbidden(String),
    Other(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Connection(e) => write!(f, "connection failed: {e}"),
            TransportError::Timeout => f.write_str("timed out"),
            TransportError::Forbidden(e) => write!(f, "request not allowed: {e}"),
            TransportError::Other(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for TransportError {}

pub trait Transport: Send + Sync {
    /// GET `url`. When `range_start > 0` a `Range: bytes=<start>-` header is
    /// sent, with `If-Range: <validator>` when a validator is known.
    fn get(
        &self,
        url: &str,
        range_start: u64,
        if_range: Option<&str>,
    ) -> Result<Response, TransportError>;
}

#[derive(Clone, Debug)]
pub struct TransportConfig {
    pub connect_timeout: Duration,
    /// Time allowed to receive response headers.
    pub response_timeout: Duration,
    /// Budget for one body transfer. When it expires the downloader simply
    /// resumes from the current offset, so this bounds how long a stalled
    /// connection can hang without limiting large downloads.
    pub body_budget: Duration,
    pub proxy: ProxySetting,
    /// Allow `http://` URLs. Only for loopback testing; production
    /// downloads are HTTPS-only.
    pub allow_insecure_http: bool,
    pub max_redirects: u32,
    pub user_agent: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxySetting {
    /// `HTTPS_PROXY` / `ALL_PROXY` from the environment.
    FromEnvironment,
    None,
    Url(String),
}

impl Default for TransportConfig {
    fn default() -> Self {
        TransportConfig {
            connect_timeout: Duration::from_secs(15),
            response_timeout: Duration::from_secs(30),
            body_budget: Duration::from_secs(120),
            proxy: ProxySetting::FromEnvironment,
            allow_insecure_http: false,
            max_redirects: 5,
            user_agent: format!(
                "{}/{}",
                inst_brand::RUNTIME_NAME.replace(' ', "-"),
                env!("CARGO_PKG_VERSION")
            ),
        }
    }
}

#[cfg(feature = "ureq")]
pub use self::ureq_impl::UreqTransport;

#[cfg(feature = "ureq")]
mod ureq_impl {
    use super::*;
    use ureq::tls::{RootCerts, TlsConfig};

    pub struct UreqTransport {
        agent: ureq::Agent,
        allow_insecure_http: bool,
    }

    impl UreqTransport {
        pub fn new(config: &TransportConfig) -> Result<Self, TransportError> {
            let proxy = match &config.proxy {
                ProxySetting::FromEnvironment => ureq::Proxy::try_from_env(),
                ProxySetting::None => None,
                ProxySetting::Url(url) => {
                    Some(ureq::Proxy::new(url).map_err(|e| TransportError::Other(e.to_string()))?)
                }
            };
            let tls = TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build();
            let agent = ureq::Agent::config_builder()
                .timeout_connect(Some(config.connect_timeout))
                .timeout_recv_response(Some(config.response_timeout))
                .timeout_recv_body(Some(config.body_budget))
                .http_status_as_error(false)
                .https_only(!config.allow_insecure_http)
                .max_redirects(config.max_redirects)
                .proxy(proxy)
                .tls_config(tls)
                .user_agent(config.user_agent.as_str())
                .build()
                .new_agent();
            Ok(UreqTransport {
                agent,
                allow_insecure_http: config.allow_insecure_http,
            })
        }
    }

    fn classify(e: ureq::Error) -> TransportError {
        match e {
            ureq::Error::Timeout(_) => TransportError::Timeout,
            ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
                TransportError::Timeout
            }
            ureq::Error::RequireHttpsOnly(u) => TransportError::Forbidden(u),
            other => TransportError::Connection(other.to_string()),
        }
    }

    /// Parses `Content-Range: bytes <start>-<end>/<total|*>`.
    fn parse_content_range(v: &str) -> Option<(u64, Option<u64>)> {
        let rest = v.trim().strip_prefix("bytes ")?;
        let (range, total) = rest.split_once('/')?;
        let (start, _end) = range.split_once('-')?;
        let start = start.trim().parse().ok()?;
        let total = total.trim().parse().ok();
        Some((start, total))
    }

    impl Transport for UreqTransport {
        fn get(
            &self,
            url: &str,
            range_start: u64,
            if_range: Option<&str>,
        ) -> Result<Response, TransportError> {
            if !self.allow_insecure_http && !url.starts_with("https://") {
                return Err(TransportError::Forbidden(url.to_owned()));
            }
            let mut req = self.agent.get(url).header("Accept-Encoding", "identity");
            if range_start > 0 {
                req = req.header("Range", format!("bytes={range_start}-"));
                if let Some(v) = if_range {
                    req = req.header("If-Range", v);
                }
            }
            let resp = req.call().map_err(classify)?;
            let status = resp.status().as_u16();
            let headers = resp.headers();
            let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
            let validator = header("etag")
                .filter(|e| !e.starts_with("W/"))
                .or_else(|| header("last-modified"))
                .map(str::to_owned);
            let content_len: Option<u64> =
                header("content-length").and_then(|v| v.trim().parse().ok());
            let (range_start, total_len) = if status == 206 {
                match header("content-range").and_then(parse_content_range) {
                    Some((start, total)) => (start, total),
                    None => return Err(TransportError::Other("206 without Content-Range".into())),
                }
            } else {
                (0, content_len)
            };
            let body = resp.into_body().into_reader();
            Ok(Response {
                status,
                range_start,
                total_len,
                validator,
                body: Box::new(body),
            })
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn content_range() {
            assert_eq!(
                super::parse_content_range("bytes 100-199/1000"),
                Some((100, Some(1000)))
            );
            assert_eq!(super::parse_content_range("bytes 5-9/*"), Some((5, None)));
            assert_eq!(super::parse_content_range("items 1-2/3"), None);
        }
    }
}
