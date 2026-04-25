use crate::error::{Error, Result};
use url::Url;

pub trait IntoUrl: Sized {
    fn into_url(self) -> Result<Url>;
    fn as_str(&self) -> &str;
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
        Url::parse(self).map_err(|e| Error::Url(e.to_string()))
    }
    fn as_str(&self) -> &str {
        self
    }
}

impl IntoUrl for &String {
    fn into_url(self) -> Result<Url> {
        Url::parse(self).map_err(|e| Error::Url(e.to_string()))
    }
    fn as_str(&self) -> &str {
        String::as_str(self)
    }
}

impl IntoUrl for String {
    fn into_url(self) -> Result<Url> {
        Url::parse(&self).map_err(|e| Error::Url(e.to_string()))
    }
    fn as_str(&self) -> &str {
        String::as_str(self)
    }
}
