//! TCP / TLS / SOCKS5 / HTTP-proxy connector

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};

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
    Plain(BoxedIo),
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

struct PrefixedIo<T> {
    prefix: io::Cursor<Vec<u8>>,
    inner: T,
}

impl<T> PrefixedIo<T> {
    fn new(prefix: Vec<u8>, inner: T) -> Self {
        Self {
            prefix: io::Cursor::new(prefix),
            inner,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PrefixedIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.prefix.position() < self.prefix.get_ref().len() as u64 {
            let position = self.prefix.position();
            let remaining = &self.prefix.get_ref()[position as usize..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.prefix.set_position(position + n as u64);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for PrefixedIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
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
            None => BoxedIo::new(self.direct(&host, port).await?),
        };

        let final_io = if is_https {
            let server_name = ServerName::try_from(host.clone())
                .map_err(|e| Error::Tls(format!("invalid server name: {e}")))?;
            // The TCP-connect timeout above doesn't cover the TLS
            // handshake, so apply the same budget here to avoid hangs on
            // half-open or misconfigured TLS servers
            let handshake = self.tls_connector().connect(server_name, stream);
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
        let addrs = self.resolver.resolve(host).await?;
        // RFC 8305 (Happy Eyeballs v2)
        let ordered = interleave_by_family(addrs.map(|a| SocketAddr::new(a.ip(), port)));
        if ordered.is_empty() {
            return Err(Error::Connect(format!("no addresses for {host}")));
        }
        self.happy_eyeballs(host, ordered).await
    }

    /// Staggered-parallel connect over a family-interleaved address list
    /// Each address gets its own attempt started `ATTEMPT_DELAY` after the
    /// previous one; the first stream to connect wins and the rest are dropped
    async fn happy_eyeballs(&self, host: &str, addrs: Vec<SocketAddr>) -> Result<TcpStream, Error> {
        /// Connection Attempt Delay
        const ATTEMPT_DELAY: Duration = Duration::from_millis(300);

        let timeout = self.connect_timeout;
        let mut in_flight = FuturesUnordered::new();
        let mut remaining = addrs.into_iter();
        let mut last: Option<io::Error> = None;

        // Prime the first attempt
        if let Some(addr) = remaining.next() {
            in_flight.push(connect_one(addr, timeout));
        }

        let stagger = tokio::time::sleep(ATTEMPT_DELAY);
        tokio::pin!(stagger);

        loop {
            // Wait for either the next in-flight attempt to finish or the
            // stagger timer to fire, whichever comes first
            tokio::select! {
                biased;
                finished = in_flight.next(), if !in_flight.is_empty() => {
                    match finished {
                        Some(Ok((stream, addr))) => {
                            self.tune(&stream)?;
                            tracing::debug!(
                                "connected host={host} peer={addr} family={}",
                                if addr.is_ipv6() { "v6" } else { "v4" },
                            );
                            return Ok(stream);
                        }
                        Some(Err(e)) => {
                            last = Some(e);
                            // That attempt failed
                            if in_flight.is_empty() {
                                if let Some(addr) = remaining.next() {
                                    in_flight.push(connect_one(addr, timeout));
                                } else {
                                    break;
                                }
                            }
                        }
                        None => {
                            // No attempts in flight and the stream drained
                            if let Some(addr) = remaining.next() {
                                in_flight.push(connect_one(addr, timeout));
                            } else {
                                break;
                            }
                        }
                    }
                }
                _ = &mut stagger => {
                    // Stagger elapsed without a winner
                    if let Some(addr) = remaining.next() {
                        in_flight.push(connect_one(addr, timeout));
                    } else if in_flight.is_empty() {
                        break;
                    }
                    stagger.as_mut().reset(tokio::time::Instant::now() + ATTEMPT_DELAY);
                }
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
    ) -> Result<BoxedIo, Error> {
        match proxy.scheme() {
            ProxyScheme::Http => {
                let phost = proxy
                    .url()
                    .host_str()
                    .ok_or_else(|| Error::Url("proxy missing host".into()))?
                    .to_string();
                let pport = proxy.url().port().unwrap_or(80);
                let stream = self.direct(&phost, pport).await?;
                if is_https {
                    http_connect(stream, host, port, proxy, self.connect_timeout).await
                } else {
                    Ok(BoxedIo::new(stream))
                }
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
                let auth = (!user.is_empty()).then_some((user.as_str(), pass.as_str()));

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
                Ok(BoxedIo::new(stream))
            }
        }
    }
}

async fn http_connect(
    stream: TcpStream,
    host: &str,
    port: u16,
    proxy: &Proxy,
    timeout: Option<Duration>,
) -> Result<BoxedIo, Error> {
    let fut = http_connect_inner(stream, host, port, proxy);
    match timeout {
        Some(d) => tokio::time::timeout(d, fut)
            .await
            .map_err(|_| Error::Connect("proxy CONNECT timeout".into()))?,
        None => fut.await,
    }
}

async fn http_connect_inner(
    mut stream: TcpStream,
    host: &str,
    port: u16,
    proxy: &Proxy,
) -> Result<BoxedIo, Error> {
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

    let mut buf = Vec::with_capacity(1024);
    loop {
        let mut chunk = [0u8; 1024];
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| Error::Connect(format!("CONNECT read: {e}")))?;
        if n == 0 {
            return Err(Error::Connect("proxy closed during CONNECT".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
        let header_end = find_header_end(&buf);
        if header_end.is_some() {
            break;
        }
        if buf.len() > 16 * 1024 {
            return Err(Error::Connect("CONNECT response too large".into()));
        }
    }
    let header_end = find_header_end(&buf)
        .ok_or_else(|| Error::Connect("CONNECT response missing header terminator".into()))?;
    let leftover = buf.split_off(header_end);
    let head = std::str::from_utf8(&buf).map_err(|e| Error::Connect(e.to_string()))?;
    let status_line = head.lines().next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    let _http = parts.next();
    let code = parts.next().unwrap_or("");
    if !code.starts_with('2') {
        return Err(Error::Connect(format!("CONNECT failed: {status_line}")));
    }
    Ok(BoxedIo::new(PrefixedIo::new(leftover, stream)))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

/// Connect to a single address with the optional per-attempt timeout
async fn connect_one(
    addr: SocketAddr,
    timeout: Option<Duration>,
) -> Result<(TcpStream, SocketAddr), io::Error> {
    let fut = TcpStream::connect(addr);
    let stream = match timeout {
        Some(d) => tokio::time::timeout(d, fut)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timeout"))??,
        None => fut.await?,
    };
    Ok((stream, addr))
}

/// Interleave addresses by family with IPv6 first
fn interleave_by_family(addrs: impl Iterator<Item = SocketAddr>) -> Vec<SocketAddr> {
    let mut v6: std::collections::VecDeque<SocketAddr> = std::collections::VecDeque::new();
    let mut v4: std::collections::VecDeque<SocketAddr> = std::collections::VecDeque::new();
    for a in addrs {
        if a.is_ipv6() {
            v6.push_back(a);
        } else {
            v4.push_back(a);
        }
    }
    let mut out = Vec::with_capacity(v6.len() + v4.len());
    // IPv6 first, then alternate families
    while !v6.is_empty() || !v4.is_empty() {
        if let Some(a) = v6.pop_front() {
            out.push(a);
        }
        if let Some(a) = v4.pop_front() {
            out.push(a);
        }
    }
    out
}

/// Percent-decode a URL component (lossy: invalid UTF-8 is replaced).
/// `url::Url::username()` / `password()` return the raw encoded form, but
/// proxy auth needs the decoded bytes
fn percent_decode_str(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// SOCKS5 connect with optional local DNS and username/password auth
///
/// Split out so the connect-timeout wrapper covers every path
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
        // Use the configured resolver instead of bypassing split-horizon or DoH on SOCKS5
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::net::TcpListener;

    fn v4(n: u8) -> SocketAddr {
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)), 443)
    }
    fn v6(n: u16) -> SocketAddr {
        SocketAddr::new(
            std::net::IpAddr::V6(Ipv6Addr::new(0x2606, 0, 0, 0, 0, 0, 0, n)),
            443,
        )
    }

    #[test]
    fn interleave_puts_ipv6_first() {
        let input = vec![v4(1), v4(2), v6(1), v6(2)];
        let out = interleave_by_family(input.into_iter());
        // v6, v4, v6, v4
        assert_eq!(out, vec![v6(1), v4(1), v6(2), v4(2)]);
    }

    #[test]
    fn interleave_v4_only_preserves_order() {
        let input = vec![v4(1), v4(2), v4(3)];
        let out = interleave_by_family(input.into_iter());
        assert_eq!(out, vec![v4(1), v4(2), v4(3)]);
    }

    #[test]
    fn interleave_v6_only_preserves_order() {
        let input = vec![v6(1), v6(2)];
        let out = interleave_by_family(input.into_iter());
        assert_eq!(out, vec![v6(1), v6(2)]);
    }

    #[test]
    fn interleave_uneven_drains_remainder() {
        // More v6 than v4: after the pair runs out, remaining v6 trail
        let input = vec![v6(1), v6(2), v6(3), v4(1)];
        let out = interleave_by_family(input.into_iter());
        assert_eq!(out, vec![v6(1), v4(1), v6(2), v6(3)]);
    }

    fn test_connector() -> Connector {
        let roots = rustls::RootCertStore::empty();
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Connector {
            tls: Arc::new(tls),
            resolver: Arc::new(crate::resolver::GaiResolver),
            proxy: None,
            connect_timeout: Some(Duration::from_secs(2)),
            tcp_nodelay: true,
            tcp_keepalive: None,
        }
    }

    #[tokio::test]
    async fn happy_eyeballs_connects_to_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let c = test_connector();
        let stream = c.happy_eyeballs("localhost", vec![addr]).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap(), addr);
    }

    #[tokio::test]
    async fn happy_eyeballs_fails_over_to_second_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = listener.local_addr().unwrap();
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let c = test_connector();
        let stream = c
            .happy_eyeballs("localhost", vec![dead, live])
            .await
            .unwrap();
        assert_eq!(stream.peer_addr().unwrap(), live);
    }

    #[tokio::test]
    async fn happy_eyeballs_all_fail_returns_error() {
        let dead1: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let dead2: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let c = test_connector();
        let res = c.happy_eyeballs("localhost", vec![dead1, dead2]).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn http_connect_preserves_bytes_read_after_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut req = [0u8; 256];
            let n = socket.read(&mut req).await.unwrap();
            assert!(std::str::from_utf8(&req[..n])
                .unwrap()
                .starts_with("CONNECT example.com:443 HTTP/1.1"));
            socket
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\nhello")
                .await
                .unwrap();
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let proxy = Proxy::all("http://127.0.0.1:8080").unwrap();
        let mut tunneled = http_connect(stream, "example.com", 443, &proxy, None)
            .await
            .unwrap();
        let mut out = [0u8; 5];
        tunneled.read_exact(&mut out).await.unwrap();

        assert_eq!(&out, b"hello");
        server.await.unwrap();
    }
}
