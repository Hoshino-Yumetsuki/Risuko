use crate::error::{Error, Result};
use url::Url;

pub trait IntoUrl: Sized {
    fn into_url(self) -> Result<Url>;
    fn as_str(&self) -> &str;
}

/// Centralised parse helper so the `&str`/`&String`/`String` impls below
/// don't each spell out the same `Url::parse(...).map_err(...)` chain
fn parse_url(s: &str) -> Result<Url> {
    Url::parse(s).map_err(|e| Error::Url(e.to_string()))
}

impl IntoUrl for Url {
    fn into_url(self) -> Result<Url> {
        Ok(self)
    }
    fn as_str(&self) -> &str {
        Url::as_str(self)
    }
}

impl IntoUrl for &Url {
    fn into_url(self) -> Result<Url> {
        Ok(self.clone())
    }
    fn as_str(&self) -> &str {
        Url::as_str(self)
    }
}

impl IntoUrl for &str {
    fn into_url(self) -> Result<Url> {
        parse_url(self)
    }
    fn as_str(&self) -> &str {
        self
    }
}

impl IntoUrl for &String {
    fn into_url(self) -> Result<Url> {
        parse_url(self)
    }
    fn as_str(&self) -> &str {
        String::as_str(self)
    }
}

impl IntoUrl for String {
    fn into_url(self) -> Result<Url> {
        parse_url(&self)
    }
    fn as_str(&self) -> &str {
        String::as_str(self)
    }
}
