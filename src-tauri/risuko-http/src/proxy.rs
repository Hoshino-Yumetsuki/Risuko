use crate::error::{Error, Result};
use url::Url;

/// Proxy configuration. Only "all-traffic" mode is supported (matches the
/// single use site in Risuko: `--all-proxy`)
#[derive(Clone, Debug)]
pub struct Proxy {
    pub(crate) url: Url,
    pub(crate) scheme: ProxyScheme,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProxyScheme {
    Http,
    Socks5 { resolve_locally: bool },
}

impl Proxy {
    pub fn all<U: AsRef<str>>(url: U) -> Result<Self> {
        let url = Url::parse(url.as_ref()).map_err(|e| Error::Url(e.to_string()))?;
        let scheme = match url.scheme() {
            "http" => ProxyScheme::Http,
            // HTTPS proxies require a TLS handshake to the proxy *before*
            // the CONNECT tunnel, which the connector doesn't yet implement
            // Reject the scheme outright so credentials aren't accidentally
            // sent in plaintext to a server that was expected to be TLS
            "https" => {
                return Err(Error::Url(
                    "https:// proxies are not yet supported; use http:// or socks5://".into(),
                ))
            }
            "socks5" => ProxyScheme::Socks5 {
                resolve_locally: true,
            },
            "socks5h" => ProxyScheme::Socks5 {
                resolve_locally: false,
            },
            other => return Err(Error::Url(format!("unsupported proxy scheme: {other}"))),
        };
        Ok(Self { url, scheme })
    }

    pub(crate) fn url(&self) -> &Url {
        &self.url
    }

    pub(crate) fn scheme(&self) -> &ProxyScheme {
        &self.scheme
    }
}
