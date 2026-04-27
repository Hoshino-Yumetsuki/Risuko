//! HTTP client used by Risuko

pub mod body;
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

pub use body::ReqBody;
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

/// Helper: build a streaming `ReqBody` that reads from a fresh `tokio::fs::File`
/// each time it is polled. Suitable for large file uploads (WebDAV PUT, S3 PUT)
///
/// The path is captured by the factory; on every send (including redirect/retry
/// replays) a brand-new `File` is opened so the stream can be re-driven from
/// byte zero
pub fn file_stream_body(path: std::path::PathBuf, content_length: Option<u64>) -> ReqBody {
    use futures_util::stream::TryStreamExt;
    use http_body::Frame;
    use http_body_util::{combinators::BoxBody, StreamBody};

    ReqBody::from_stream(
        move || {
            let path = path.clone();
            // Lazily open the file the first time the body is polled. Any IO
            // error becomes a body error — hyper will surface it to the caller
            let stream = async_stream::try_stream! {
                let file = tokio::fs::File::open(&path).await
                    .map_err(|e| Error::Body(format!("open {}: {e}", path.display())))?;
                let reader = tokio_util::io::ReaderStream::new(file);
                let mut reader = std::pin::pin!(reader);
                use futures_util::StreamExt;
                while let Some(chunk) = reader.next().await {
                    let bytes = chunk.map_err(|e| Error::Body(e.to_string()))?;
                    yield bytes;
                }
            };
            let frame_stream = stream.map_ok(Frame::data);
            BoxBody::new(StreamBody::new(frame_stream))
        },
        content_length,
    )
}
