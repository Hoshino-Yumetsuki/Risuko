//! TCP / TLS / SOCKS5 / HTTP-proxy connector

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64_STANDARD;
use base64::Engine as _;
use http::Uri;
use hyper::rt::ReadBufCursor;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tower_service::Service;

use crate::error::Error;
use crate::proxy::{Proxy, ProxyScheme};
use crate::resolver::SharedResolver;

#[derive(Clone)]
pub(crate) struct Connector {
    pub(crate) tls: Arc<ClientConfig>,
    pub(crate) resolver: SharedResolver,
    pub(crate) proxy: Option<Arc<Proxy>>,
    pub(crate) connect_timeout: Option<Duration>,
    pub(crate) tcp_nodelay: bool,
    pub(crate) tcp_keepalive: Option<Duration>,
}

impl Connector {
    fn tls_connector(&self) -> TlsConnector {
        TlsConnector::from(self.tls.clone())
    }
}

pub(crate) enum MaybeTls {
    Plain(TcpStream),
    Tls(Box<TlsStream<BoxedIo>>),
}

/// Type-erased AsyncRead+AsyncWrite stream so TLS can sit on top of either
/// TCP or a SOCKS-tunneled TCP without compile-time enum gymnastics
pub struct BoxedIo {
    inner: Box<dyn AsyncReadWrite + Send + Unpin>,
}

trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> AsyncReadWrite for T {}

impl BoxedIo {
    fn new<T: AsyncRead + AsyncWrite + Send + Unpin + 'static>(t: T) -> Self {
        Self { inner: Box::new(t) }
    }
}

impl AsyncRead for BoxedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for BoxedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Hyper-compatible IO wrapper around `MaybeTls`
pub struct ConnIo {
    io: TokioIo<MaybeTls>,
}

impl Connection for ConnIo {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl hyper::rt::Read for ConnIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for ConnIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

impl AsyncRead for MaybeTls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

impl Service<Uri> for Connector {
    type Response = ConnIo;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<ConnIo, Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let this = self.clone();
        Box::pin(async move { this.connect(dst).await })
    }
}

impl Connector {
    async fn connect(self, dst: Uri) -> Result<ConnIo, Error> {
        let scheme = dst.scheme_str().unwrap_or("http").to_string();
        let host = dst
            .host()
            .ok_or_else(|| Error::Url("missing host".into()))?
            .to_string();
        let port = dst.port_u16().unwrap_or(match scheme.as_str() {
            "https" => 443,
            _ => 80,
        });
        let is_https = scheme == "https";

        let stream = match self.proxy.as_deref() {
            Some(p) => self.via_proxy(p, &host, port, is_https).await?,
            None => self.direct(&host, port).await?,
        };

        let final_io = if is_https {
            let server_name = ServerName::try_from(host.clone())
                .map_err(|e| Error::Tls(format!("invalid server name: {e}")))?;
            // The TCP-connect timeout above doesn't cover the TLS
            // handshake, so apply the same budget here to avoid hangs on
            // half-open or misconfigured TLS servers
            let handshake = self
                .tls_connector()
                .connect(server_name, BoxedIo::new(stream));
            let tls = match self.connect_timeout {
                Some(d) => tokio::time::timeout(d, handshake)
                    .await
                    .map_err(|_| Error::Tls("TLS handshake timeout".into()))?
                    .map_err(|e| Error::Tls(e.to_string()))?,
                None => handshake.await.map_err(|e| Error::Tls(e.to_string()))?,
            };
            MaybeTls::Tls(Box::new(tls))
        } else {
            MaybeTls::Plain(stream)
        };

        Ok(ConnIo {
            io: TokioIo::new(final_io),
        })
    }

    async fn direct(&self, host: &str, port: u16) -> Result<TcpStream, Error> {
        let mut addrs = self.resolver.resolve(host).await?;
        let mut last: Option<io::Error> = None;
        let timeout = self.connect_timeout;
        while let Some(addr) = addrs.next() {
            let addr = SocketAddr::new(addr.ip(), port);
            let fut = TcpStream::connect(addr);
            let res = match timeout {
                Some(d) => match tokio::time::timeout(d, fut).await {
                    Ok(r) => r,
                    Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "connect timeout")),
                },
                None => fut.await,
            };
            match res {
                Ok(s) => {
                    self.tune(&s)?;
                    return Ok(s);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(Error::Connect(
            last.map(|e| e.to_string())
                .unwrap_or_else(|| format!("no addresses for {host}")),
        ))
    }

    fn tune(&self, s: &TcpStream) -> Result<(), Error> {
        if self.tcp_nodelay {
            let _ = s.set_nodelay(true);
        }
        if let Some(d) = self.tcp_keepalive {
            let sock = socket2::SockRef::from(s);
            let ka = socket2::TcpKeepalive::new().with_time(d);
            let _ = sock.set_tcp_keepalive(&ka);
        }
        Ok(())
    }

    async fn via_proxy(
        &self,
        proxy: &Proxy,
        host: &str,
        port: u16,
        is_https: bool,
    ) -> Result<TcpStream, Error> {
        match proxy.scheme() {
            ProxyScheme::Http => {
                let phost = proxy
                    .url()
                    .host_str()
                    .ok_or_else(|| Error::Url("proxy missing host".into()))?
                    .to_string();
                let pport = proxy.url().port().unwrap_or(80);
                let mut stream = self.direct(&phost, pport).await?;
                if is_https {
                    http_connect(&mut stream, host, port, proxy, self.connect_timeout).await?;
                }
                Ok(stream)
            }
            ProxyScheme::Socks5 { resolve_locally } => {
                let phost = proxy
                    .url()
                    .host_str()
                    .ok_or_else(|| Error::Url("proxy missing host".into()))?
                    .to_string();
                let pport = proxy.url().port().unwrap_or(1080);
                // Proxy credentials in URLs are percent-encoded
                // (RFC 3986). Decode before handing to either the SOCKS5
                // username/password auth method or HTTP `Basic` so creds
                // containing `@`, `:`, spaces, etc. authenticate correctly
                let user_raw = proxy.url().username();
                let pass_raw = proxy.url().password().unwrap_or("");
                let user = percent_decode_str(user_raw);
                let pass = percent_decode_str(pass_raw);
                let target_addr_str = format!("{host}:{port}");
                let proxy_addr = format!("{phost}:{pport}");
                let auth = (!user.is_empty()).then(|| (user.as_str(), pass.as_str()));

                // Apply the same connect-timeout budget to SOCKS5 as to
                // direct/HTTP-CONNECT paths so a misbehaving proxy can't
                // hang the worker indefinitely
                let stream = match self.connect_timeout {
                    Some(d) => tokio::time::timeout(
                        d,
                        socks5_connect(
                            &proxy_addr,
                            &target_addr_str,
                            host,
                            port,
                            *resolve_locally,
                            auth,
                            &self.resolver,
                        ),
                    )
                    .await
                    .map_err(|_| Error::Connect("SOCKS5 connect timeout".into()))??,
                    None => {
                        socks5_connect(
                            &proxy_addr,
                            &target_addr_str,
                            host,
                            port,
                            *resolve_locally,
                            auth,
                            &self.resolver,
                        )
                        .await?
                    }
                };
                self.tune(&stream)?;
                Ok(stream)
            }
        }
    }
}

async fn http_connect(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
    proxy: &Proxy,
    timeout: Option<Duration>,
) -> Result<(), Error> {
    let fut = http_connect_inner(stream, host, port, proxy);
    match timeout {
        Some(d) => tokio::time::timeout(d, fut)
            .await
            .map_err(|_| Error::Connect("proxy CONNECT timeout".into()))?,
        None => fut.await,
    }
}

async fn http_connect_inner(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
    proxy: &Proxy,
) -> Result<(), Error> {
    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if !proxy.url().username().is_empty() {
        // Percent-decode credentials per RFC 3986 before encoding to Basic;
        // otherwise creds containing reserved characters (e.g. `@`, `:`,
        // space) authenticate against the wrong literal value
        let user = percent_decode_str(proxy.url().username());
        let pass = percent_decode_str(proxy.url().password().unwrap_or(""));
        let creds = format!("{user}:{pass}");
        let encoded = B64_STANDARD.encode(creds.as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    req.push_str("\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| Error::Connect(format!("CONNECT write: {e}")))?;

    let mut buf = Vec::with_capacity(256);
    loop {
        let mut byte = [0u8; 1];
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| Error::Connect(format!("CONNECT read: {e}")))?;
        if n == 0 {
            return Err(Error::Connect("proxy closed during CONNECT".into()));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            return Err(Error::Connect("CONNECT response too large".into()));
        }
    }
    let head = std::str::from_utf8(&buf).map_err(|e| Error::Connect(e.to_string()))?;
    let status_line = head.lines().next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    let _http = parts.next();
    let code = parts.next().unwrap_or("");
    if !code.starts_with('2') {
        return Err(Error::Connect(format!("CONNECT failed: {status_line}")));
    }
    Ok(())
}

/// Percent-decode a URL component (lossy: invalid UTF-8 is replaced).
/// `url::Url::username()` / `password()` return the raw encoded form, but
/// proxy auth needs the decoded bytes
fn percent_decode_str(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// SOCKS5 connect with optional local DNS and optional username/password
/// auth. Pulled out so the connect-timeout wrapper can apply the same
/// budget to every code path
async fn socks5_connect(
    proxy_addr: &str,
    target_addr_str: &str,
    host: &str,
    port: u16,
    resolve_locally: bool,
    auth: Option<(&str, &str)>,
    resolver: &SharedResolver,
) -> Result<TcpStream, Error> {
    if resolve_locally {
        // Honour the configured custom resolver instead of bypassing it via
        // `tokio::net::lookup_host`. Otherwise a custom DNS resolver set on
        // the client (e.g. for split-horizon or DoH) would silently be
        // skipped on the SOCKS5 path
        let target_ip = resolver
            .resolve(host)
            .await?
            .next()
            .ok_or_else(|| Error::Connect(format!("no addrs for {host}")))?
            .ip();
        let target = SocketAddr::new(target_ip, port);
        let _ = target_addr_str;
        match auth {
            Some((u, p)) => Ok(tokio_socks::tcp::Socks5Stream::connect_with_password(
                proxy_addr, target, u, p,
            )
            .await
            .map_err(|e| Error::Connect(e.to_string()))?
            .into_inner()),
            None => Ok(tokio_socks::tcp::Socks5Stream::connect(proxy_addr, target)
                .await
                .map_err(|e| Error::Connect(e.to_string()))?
                .into_inner()),
        }
    } else {
        match auth {
            Some((u, p)) => Ok(tokio_socks::tcp::Socks5Stream::connect_with_password(
                proxy_addr,
                (host, port),
                u,
                p,
            )
            .await
            .map_err(|e| Error::Connect(e.to_string()))?
            .into_inner()),
            None => Ok(
                tokio_socks::tcp::Socks5Stream::connect(proxy_addr, (host, port))
                    .await
                    .map_err(|e| Error::Connect(e.to_string()))?
                    .into_inner(),
            ),
        }
    }
}
