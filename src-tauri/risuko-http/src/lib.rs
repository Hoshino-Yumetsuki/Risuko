//! HTTP client used by Risuko

mod body;
mod client;
mod connector;
mod cookies;
mod decompress;
mod error;
mod into_url;
mod proxy;
pub mod redirect;
mod request;
mod resolver;
mod response;

pub use client::{Client, ClientBuilder};
pub use cookies::{CookieStore, Jar};
pub use error::{Error, Result};
pub use into_url::IntoUrl;
pub use proxy::Proxy;
pub use redirect::Policy;
pub use request::RequestBuilder;
pub use resolver::{Addrs, Resolve, Resolving};
pub use response::Response;

pub use http::header;
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
pub use url::Url;
