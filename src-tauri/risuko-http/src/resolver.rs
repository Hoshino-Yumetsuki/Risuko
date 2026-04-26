use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::{Error, Result};

/// Iterator over resolved socket addresses
pub type Addrs = Box<dyn Iterator<Item = SocketAddr> + Send>;

/// Future returned by a `Resolve` implementation
pub type Resolving = Pin<Box<dyn Future<Output = Result<Addrs>> + Send>>;

/// Pluggable DNS resolver. The default implementation uses
/// `tokio::net::lookup_host`
///
/// # Port-0 contract
///
/// Implementations resolve a *bare hostname* (no `:port`). The built-in
/// [`GaiResolver`] feeds `tokio::net::lookup_host` a `(host, 0)` tuple, so
/// every [`Addrs`] entry it produces has port `0`. Callers (the connector)
/// are responsible for substituting the real destination port via
/// `SocketAddr::new(addr.ip(), port)` before connecting. Third-party
/// implementers must not parse a `"host:port"` string out of `host` and
/// must not embed a meaningful port in the returned [`SocketAddr`]s
pub trait Resolve: Send + Sync {
    /// Resolve `host` to one or more [`SocketAddr`]s. Per the trait-level
    /// contract, the port field of each returned address is unspecified and
    /// must be overwritten by the caller before connecting
    fn resolve(&self, host: &str) -> Resolving;
}

#[derive(Clone, Default)]
pub(crate) struct GaiResolver;

impl Resolve for GaiResolver {
    fn resolve(&self, host: &str) -> Resolving {
        let host = host.to_string();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(Error::Io)?;
            let v: Vec<SocketAddr> = addrs.collect();
            Ok(Box::new(v.into_iter()) as Addrs)
        })
    }
}

pub(crate) type SharedResolver = Arc<dyn Resolve>;
