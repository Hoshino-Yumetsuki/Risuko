use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{HeaderMap, HeaderValue, CONTENT_LENGTH, HOST, USER_AGENT};
use http::{Method, Request, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use url::Url;

use crate::body::ReqBody;
use crate::connector::{ConnIo, Connector};
use crate::cookies::SharedJar;
use crate::decompress;
use crate::error::{Error, Result};
use crate::into_url::IntoUrl;
use crate::proxy::Proxy;
use crate::redirect::Policy;
use crate::request::RequestBuilder;
use crate::resolver::{GaiResolver, Resolve, SharedResolver};
use crate::response::Response;

#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    hyper: HyperClient<Connector, BoxBody<Bytes, Error>>,
    user_agent: Option<HeaderValue>,
    default_headers: HeaderMap,
    redirect: Policy,
    timeout: Option<Duration>,
    auto_gzip: bool,
    auto_brotli: bool,
    auto_deflate: bool,
    cookie_jar: Option<SharedJar>,
    accepts: HeaderValue,
}

impl Client {
    pub fn new() -> Self {
        ClientBuilder::new().build().expect("default client build")
    }

    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::GET, url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::POST, url)
    }

    pub fn put<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PUT, url)
    }

    pub fn delete<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::DELETE, url)
    }

    pub fn head<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::HEAD, url)
    }

    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        RequestBuilder::new(self.clone(), method, url.into_url())
    }

    pub(crate) async fn execute(&self, mut rb: RequestBuilder) -> Result<Response> {
        let url = rb.url?;
        let timeout = rb.timeout.or(self.inner.timeout);

        // Start with default headers, then let request-specific headers
        // override them. Per-name override semantics: if the request sets any
        // value for a header name, all default values for that name are
        // dropped before the request's values are appended. This way a
        // request `User-Agent` replaces the default UA instead of being sent
        // alongside it (HTTP forbids duplicate singleton headers), while
        // multi-valued request headers like `Accept` still preserve all
        // values the caller appended
        let mut headers = HeaderMap::new();
        for (k, v) in self.inner.default_headers.iter() {
            headers.append(k.clone(), v.clone());
        }
        if let Some(ua) = &self.inner.user_agent {
            if !rb.headers.contains_key(USER_AGENT) {
                headers.insert(USER_AGENT, ua.clone());
            }
        }
        // `HeaderMap::drain` yields `(Option<HeaderName>, HeaderValue)`:
        // `Some(name)` introduces a new header, subsequent `None` entries
        // belong to the same name. Track the last name so multi-valued
        // request headers survive the merge, and remove any default entries
        // for names the request sets so we don't end up with duplicates
        let mut last_name: Option<http::HeaderName> = None;
        let mut overridden: std::collections::HashSet<http::HeaderName> =
            std::collections::HashSet::new();
        for (k, v) in rb.headers.drain() {
            let name = match k {
                Some(n) => {
                    last_name = Some(n.clone());
                    n
                }
                None => match last_name.clone() {
                    Some(n) => n,
                    None => continue,
                },
            };
            if overridden.insert(name.clone()) {
                headers.remove(&name);
            }
            headers.append(name, v);
        }

        let body = std::mem::replace(&mut rb.body, ReqBody::Empty);
        let fut = self.send_with_redirects(rb.method, url, headers, body);
        match timeout {
            Some(d) => tokio::time::timeout(d, fut)
                .await
                .map_err(|_| Error::Timeout)?,
            None => fut.await,
        }
    }

    async fn send_with_redirects(
        &self,
        method: Method,
        mut url: Url,
        mut headers: HeaderMap,
        mut body: ReqBody,
    ) -> Result<Response> {
        let mut method = method;
        let mut redirects = 0usize;

        loop {
            let resp = self
                .send_once(&method, &url, &headers, body.clone())
                .await?;
            let status = resp.status();
            if !is_redirect_status(status) || !self.inner.redirect.follows() {
                return Ok(resp);
            }
            redirects += 1;
            if !self.inner.redirect.unlimited && redirects > self.inner.redirect.max {
                return match self.inner.redirect.on_exceeded {
                    crate::redirect::OnExceeded::Stop => Ok(resp),
                    crate::redirect::OnExceeded::Error => Err(Error::Redirect(format!(
                        "exceeded {} redirects",
                        self.inner.redirect.max
                    ))),
                };
            }

            let location = resp
                .headers()
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Error::Redirect("missing Location header".into()))?
                .to_string();
            let next = url
                .join(&location)
                .map_err(|e| Error::Redirect(e.to_string()))?;

            // RFC 7231 §6.4: 301/302/303 may downgrade method to GET
            // 307/308 preserve method and body
            if matches!(
                status,
                StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
            ) && method != Method::GET
                && method != Method::HEAD
            {
                method = Method::GET;
                body = ReqBody::Empty;
                headers.remove(CONTENT_LENGTH);
                headers.remove(http::header::CONTENT_TYPE);
                // Strip caller-supplied body-framing headers too. Forwarding
                // `Transfer-Encoding: chunked` together with `ReqBody::Empty`
                // would produce a malformed request the upstream may reject
                // or interpret ambiguously
                headers.remove(http::header::TRANSFER_ENCODING);
            }

            // Drop sensitive headers on cross-origin redirect
            if !same_origin(&url, &next) {
                headers.remove(http::header::AUTHORIZATION);
                headers.remove(http::header::COOKIE);
                // Always strip an explicit Host so `send_once` can
                // regenerate it from the new origin. Otherwise an explicit
                // Host (set by the caller or via `default_headers`) would
                // leak the old origin to the redirect target
                headers.remove(HOST);
            }

            url = next;
        }
    }

    async fn send_once(
        &self,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        body: ReqBody,
    ) -> Result<Response> {
        let uri: Uri = url
            .as_str()
            .parse()
            .map_err(|e: http::uri::InvalidUri| Error::Url(e.to_string()))?;

        let mut builder = Request::builder().method(method.clone()).uri(uri);
        let req_headers = builder
            .headers_mut()
            .ok_or_else(|| Error::Builder("no headers".into()))?;
        // `HeaderMap::iter` yields the header name on every entry (unlike
        // `drain`, which only yields it on the first entry of a group), so
        // appending each (k, v) is enough to preserve multi-valued headers
        for (k, v) in headers.iter() {
            req_headers.append(k.clone(), v.clone());
        }
        // Hyper requires a Host header; fill from URL if absent
        if !req_headers.contains_key(HOST) {
            if let Some(host) = url.host_str() {
                let value = if let Some(port) = url.port() {
                    format!("{host}:{port}")
                } else {
                    host.to_string()
                };
                if let Ok(v) = HeaderValue::from_str(&value) {
                    req_headers.insert(HOST, v);
                }
            }
        }
        // Auto Accept-Encoding when the user enabled compression and didn't
        // override it themselves. Skip when no codec is enabled so we don't
        // emit an empty header (which servers may treat as ambiguous)
        if !self.inner.accepts.is_empty()
            && !req_headers.contains_key(http::header::ACCEPT_ENCODING)
        {
            req_headers.insert(http::header::ACCEPT_ENCODING, self.inner.accepts.clone());
        }
        // Cookie jar injection. Preserve any caller-provided Cookie header
        // (matching reqwest's behaviour) so manual overrides win over the jar
        // and we never silently clobber the user's value
        if let Some(jar) = &self.inner.cookie_jar {
            if !req_headers.contains_key(http::header::COOKIE) {
                if let Some(cookie) = jar.cookies(url) {
                    req_headers.insert(http::header::COOKIE, cookie);
                }
            }
        }

        // For streaming bodies with a known length, advertise Content-Length
        // when the caller didn't already set framing headers. Without either
        // Content-Length or Transfer-Encoding hyper would buffer the entire
        // body to compute one — defeating streaming. Bytes/Empty bodies are
        // sized automatically by hyper from the body's `size_hint`
        if matches!(body, ReqBody::Stream { .. }) {
            let has_length = req_headers.contains_key(CONTENT_LENGTH);
            // RFC 9112: a request that already carries Transfer-Encoding
            // *must not* also advertise Content-Length, regardless of which
            // codings appear (the final coding is implicitly chunked). Detect
            // both an explicit "chunked" token and the broader "any TE
            // header present at all" case so we never inject a conflicting
            // Content-Length on top of the caller's framing
            let has_chunked = req_headers
                .get_all(http::header::TRANSFER_ENCODING)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|s| s.split(','))
                .any(|tok| tok.trim().eq_ignore_ascii_case("chunked"));
            let has_te = req_headers.contains_key(http::header::TRANSFER_ENCODING);
            if !has_length && !has_chunked && !has_te {
                if let Some(len) = body.content_length() {
                    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
                        req_headers.insert(CONTENT_LENGTH, v);
                    }
                } else {
                    // Unknown length — let hyper send chunked
                    req_headers.insert(
                        http::header::TRANSFER_ENCODING,
                        HeaderValue::from_static("chunked"),
                    );
                }
            }
        }

        let req = builder
            .body(body.into_hyper_body())
            .map_err(|e| Error::Builder(e.to_string()))?;

        let resp = self.inner.hyper.request(req).await?;

        let (parts, body) = resp.into_parts();

        // Capture Set-Cookie before we move headers into Response
        if let Some(jar) = &self.inner.cookie_jar {
            let mut iter = parts.headers.get_all(http::header::SET_COOKIE).into_iter();
            jar.set_cookies(&mut iter, url);
        }

        let encoding = parts
            .headers
            .get(http::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let should_decode = match encoding.as_deref() {
            Some("gzip" | "x-gzip") => self.inner.auto_gzip,
            Some("br") => self.inner.auto_brotli,
            Some("deflate") => self.inner.auto_deflate,
            _ => false,
        };

        let resp_body = if should_decode {
            let boxed: crate::body::RespBody = body.map_err(Error::from).boxed();
            decompress::maybe_decompress(boxed, encoding.as_deref())
        } else {
            body.map_err(Error::from).boxed()
        };

        // Strip Content-Length / Content-Encoding when we decoded so callers
        // don't trust stale lengths
        let mut response_headers = parts.headers;
        if should_decode {
            response_headers.remove(CONTENT_LENGTH);
            response_headers.remove(http::header::CONTENT_ENCODING);
        }

        Ok(Response::new(
            parts.status,
            parts.version,
            response_headers,
            url.clone(),
            resp_body,
        ))
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

fn is_redirect_status(s: StatusCode) -> bool {
    matches!(
        s,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

// --- Builder --

pub struct ClientBuilder {
    user_agent: Option<HeaderValue>,
    default_headers: HeaderMap,
    redirect: Policy,
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    pool_idle_timeout: Option<Duration>,
    pool_max_idle_per_host: Option<usize>,
    tcp_nodelay: bool,
    tcp_keepalive: Option<Duration>,
    http1_only: bool,
    auto_gzip: bool,
    auto_brotli: bool,
    auto_deflate: bool,
    danger_accept_invalid_certs: bool,
    proxy: Option<Proxy>,
    cookie_jar: Option<SharedJar>,
    resolver: Option<SharedResolver>,
    extra_root_certs: Vec<Vec<u8>>,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            user_agent: None,
            default_headers: HeaderMap::new(),
            redirect: Policy::default(),
            timeout: None,
            connect_timeout: None,
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle_per_host: None,
            tcp_nodelay: true,
            tcp_keepalive: None,
            http1_only: false,
            auto_gzip: false,
            auto_brotli: false,
            auto_deflate: false,
            danger_accept_invalid_certs: false,
            proxy: None,
            cookie_jar: None,
            resolver: None,
            extra_root_certs: Vec::new(),
        }
    }

    pub fn user_agent<V: TryInto<HeaderValue>>(mut self, ua: V) -> Self {
        self.user_agent = ua.try_into().ok();
        self
    }

    pub fn default_headers(mut self, headers: HeaderMap) -> Self {
        self.default_headers = headers;
        self
    }

    pub fn redirect(mut self, policy: Policy) -> Self {
        self.redirect = policy;
        self
    }

    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }

    pub fn connect_timeout(mut self, t: Duration) -> Self {
        self.connect_timeout = Some(t);
        self
    }

    pub fn pool_idle_timeout(mut self, t: Duration) -> Self {
        self.pool_idle_timeout = Some(t);
        self
    }

    pub fn pool_max_idle_per_host(mut self, n: usize) -> Self {
        self.pool_max_idle_per_host = Some(n);
        self
    }

    pub fn tcp_nodelay(mut self, b: bool) -> Self {
        self.tcp_nodelay = b;
        self
    }

    pub fn tcp_keepalive<D: Into<Option<Duration>>>(mut self, d: D) -> Self {
        self.tcp_keepalive = d.into();
        self
    }

    pub fn http1_only(mut self) -> Self {
        self.http1_only = true;
        self
    }

    pub fn gzip(mut self, b: bool) -> Self {
        self.auto_gzip = b;
        self
    }
    pub fn brotli(mut self, b: bool) -> Self {
        self.auto_brotli = b;
        self
    }
    pub fn deflate(mut self, b: bool) -> Self {
        self.auto_deflate = b;
        self
    }
    pub fn no_gzip(mut self) -> Self {
        self.auto_gzip = false;
        self
    }
    pub fn no_brotli(mut self) -> Self {
        self.auto_brotli = false;
        self
    }
    pub fn no_deflate(mut self) -> Self {
        self.auto_deflate = false;
        self
    }

    pub fn danger_accept_invalid_certs(mut self, b: bool) -> Self {
        self.danger_accept_invalid_certs = b;
        self
    }

    pub fn proxy(mut self, p: Proxy) -> Self {
        self.proxy = Some(p);
        self
    }

    pub fn cookie_provider(mut self, jar: Arc<dyn crate::cookies::CookieStore>) -> Self {
        self.cookie_jar = Some(jar);
        self
    }

    pub fn resolver<R: Resolve + 'static>(mut self, r: R) -> Self {
        self.resolver = Some(Arc::new(r));
        self
    }

    pub fn add_root_certificate(mut self, der: impl Into<Vec<u8>>) -> Self {
        self.extra_root_certs.push(der.into());
        self
    }

    pub fn build(self) -> Result<Client> {
        let tls = build_tls(self.danger_accept_invalid_certs, &self.extra_root_certs)?;

        let connector = Connector {
            tls: Arc::new(tls),
            resolver: self.resolver.unwrap_or_else(|| Arc::new(GaiResolver)),
            proxy: self.proxy.map(Arc::new),
            connect_timeout: self.connect_timeout,
            tcp_nodelay: self.tcp_nodelay,
            tcp_keepalive: self.tcp_keepalive,
        };

        // The connector below never advertises h2 ALPN and the plain-HTTP
        // path never initiates an h2c upgrade, so this client is HTTP/1.1
        // only at the wire level. `hyper-util 0.1`'s `legacy::Client::Builder`
        // has no dedicated `http1_only` knob, so honouring `self.http1_only`
        // is purely defensive — there is no second protocol path to disable
        let _ = self.http1_only;
        let mut hyper_builder = HyperClient::builder(TokioExecutor::new());
        if let Some(d) = self.pool_idle_timeout {
            hyper_builder.pool_idle_timeout(d);
        }
        if let Some(n) = self.pool_max_idle_per_host {
            hyper_builder.pool_max_idle_per_host(n);
        }
        let hyper = hyper_builder.build::<Connector, BoxBody<Bytes, Error>>(connector);

        let mut accepts: Vec<&str> = Vec::new();
        if self.auto_gzip {
            accepts.push("gzip");
        }
        if self.auto_brotli {
            accepts.push("br");
        }
        if self.auto_deflate {
            accepts.push("deflate");
        }
        let accepts = HeaderValue::from_str(&accepts.join(", "))
            .unwrap_or_else(|_| HeaderValue::from_static(""));

        let inner = ClientInner {
            hyper,
            user_agent: self.user_agent,
            default_headers: self.default_headers,
            redirect: self.redirect,
            timeout: self.timeout,
            auto_gzip: self.auto_gzip,
            auto_brotli: self.auto_brotli,
            auto_deflate: self.auto_deflate,
            cookie_jar: self.cookie_jar,
            accepts,
        };

        Ok(Client {
            inner: Arc::new(inner),
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn build_tls(danger_accept_invalid: bool, extra: &[Vec<u8>]) -> Result<ClientConfig> {
    // `ClientConfig::builder()` panics if no process-level CryptoProvider is
    // installed. Risuko only ever links against `rustls/ring`, so install
    // ring as the default the first time we build a client. The call is a
    // no-op once a provider is installed, so it is safe to repeat
    static INSTALL_PROVIDER: std::sync::Once = std::sync::Once::new();
    INSTALL_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for der in extra {
        let cert = CertificateDer::from(der.clone());
        roots
            .add(cert)
            .map_err(|e| Error::Tls(format!("invalid extra root: {e}")))?;
    }

    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    if danger_accept_invalid {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoCertVerifier));
    }

    Ok(config)
}

#[derive(Debug)]
struct NoCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme::*;
        vec![
            RSA_PKCS1_SHA256,
            RSA_PKCS1_SHA384,
            RSA_PKCS1_SHA512,
            ECDSA_NISTP256_SHA256,
            ECDSA_NISTP384_SHA384,
            ED25519,
            RSA_PSS_SHA256,
            RSA_PSS_SHA384,
            RSA_PSS_SHA512,
        ]
    }
}

// hyper-util's Client requires the connector's response to implement
// `Connection + hyper::rt::Read + hyper::rt::Write + Send + Unpin`. We
// provide all of those for `ConnIo` in the connector module
fn _assert_conn() {
    fn check<
        T: hyper::rt::Read
            + hyper::rt::Write
            + hyper_util::client::legacy::connect::Connection
            + Send
            + Unpin,
    >() {
    }
    check::<ConnIo>();
}
