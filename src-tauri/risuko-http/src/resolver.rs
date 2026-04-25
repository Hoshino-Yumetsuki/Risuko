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
pub trait Resolve: Send + Sync {
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
