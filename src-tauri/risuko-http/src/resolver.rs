use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

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
/// must substitute the real destination port via
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
pub struct GaiResolver;

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

/// Process-wide resolver override
///
/// Clients built without an explicit `.resolver_arc(...)` use [`GlobalResolver`],
/// which reads this slot on every `resolve()` call. So a swap takes hold
/// right away, even for clients that get built once and then cached for the
/// life of the process (think the `OnceLock` clients in the RSS, UPnP and
/// BitTorrent-tracker code). `None` means fall back to system DNS
static GLOBAL_RESOLVER: RwLock<Option<SharedResolver>> = RwLock::new(None);

/// Install the process-wide resolver, or clear it with `None`. From then on any
/// client that didn't pin its own resolver uses it. This is how DNS-over-HTTPS
/// gets wired in from the engine's config layer
pub fn set_global_resolver(resolver: Option<SharedResolver>) {
    if let Ok(mut slot) = GLOBAL_RESOLVER.write() {
        *slot = resolver;
    }
}

/// Grab a snapshot of the current global resolver, if one is set
fn global_resolver() -> Option<SharedResolver> {
    GLOBAL_RESOLVER.read().ok().and_then(|s| s.clone())
}

/// Default resolver for clients that don't pin one. Uses the
/// [`set_global_resolver`] override when it's set, otherwise system DNS. It
/// re-reads the slot every call, which is what lets a runtime config change
/// reach clients that were already built
#[derive(Clone, Default)]
pub(crate) struct GlobalResolver;

impl Resolve for GlobalResolver {
    fn resolve(&self, host: &str) -> Resolving {
        match global_resolver() {
            Some(r) => r.resolve(host),
            None => GaiResolver.resolve(host),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests that mutate GLOBAL_RESOLVER must run serially to avoid flaky failures
    // when cargo test runs them in parallel
    #[test]
    #[serial_test::serial]
    fn global_resolver_defaults_to_system() {
        // With nothing installed, GlobalResolver falls through to GaiResolver;
        // check the slot is empty rather than hitting the network
        set_global_resolver(None);
        assert!(global_resolver().is_none());
    }

    struct StaticResolver(SocketAddr);
    impl Resolve for StaticResolver {
        fn resolve(&self, _host: &str) -> Resolving {
            let a = self.0;
            Box::pin(async move { Ok(Box::new(std::iter::once(a)) as Addrs) })
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn global_resolver_uses_override() {
        let addr: SocketAddr = "203.0.113.7:0".parse().unwrap();
        set_global_resolver(Some(Arc::new(StaticResolver(addr))));
        let got: Vec<SocketAddr> = GlobalResolver
            .resolve("anything.invalid")
            .await
            .unwrap()
            .collect();
        set_global_resolver(None); // put it back for the other tests
        assert_eq!(got, vec![addr]);
    }
}
