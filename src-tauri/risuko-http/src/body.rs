//! Request/response body types

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};

use crate::error::Error;

/// Factory that produces a fresh outgoing body each time it's called
///
/// Used by `ReqBody::Stream` so retries/redirects can replay the body without
/// requiring the body to be `Clone`. The factory typically opens a fresh
/// `tokio::fs::File` and wraps it in `tokio_util::io::ReaderStream`
pub type StreamBodyFactory = Arc<dyn Fn() -> BoxBody<Bytes, Error> + Send + Sync + 'static>;

/// Body type sent in outgoing requests
///
/// `Bytes` carries a fully-buffered payload (small JSON/forms). `Stream`
/// carries a body that is produced on demand by a factory closure — used for
/// large uploads where we don't want to read the whole file into memory. The
/// factory pattern makes the body cheaply clonable for redirect/retry while
/// still supporting one-shot streaming reads
#[derive(Clone)]
pub enum ReqBody {
    Empty,
    Bytes(Bytes),
    Stream {
        factory: StreamBodyFactory,
        content_length: Option<u64>,
    },
}

impl std::fmt::Debug for ReqBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReqBody::Empty => f.write_str("ReqBody::Empty"),
            ReqBody::Bytes(b) => f.debug_tuple("ReqBody::Bytes").field(&b.len()).finish(),
            ReqBody::Stream { content_length, .. } => f
                .debug_struct("ReqBody::Stream")
                .field("content_length", content_length)
                .finish(),
        }
    }
}

impl ReqBody {
    pub fn from_bytes(b: impl Into<Bytes>) -> Self {
        ReqBody::Bytes(b.into())
    }

    /// Build a streaming request body from a factory closure. Each invocation
    /// of `factory` must produce a fresh, full body — the closure is called
    /// once per send (including on redirect replay)
    pub fn from_stream<F>(factory: F, content_length: Option<u64>) -> Self
    where
        F: Fn() -> BoxBody<Bytes, Error> + Send + Sync + 'static,
    {
        ReqBody::Stream {
            factory: Arc::new(factory),
            content_length,
        }
    }

    /// Content-Length hint, when known
    pub fn content_length(&self) -> Option<u64> {
        match self {
            ReqBody::Empty => Some(0),
            ReqBody::Bytes(b) => Some(b.len() as u64),
            ReqBody::Stream { content_length, .. } => *content_length,
        }
    }

    pub(crate) fn into_hyper_body(self) -> BoxBody<Bytes, Error> {
        match self {
            ReqBody::Empty => http_body_util::Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
            ReqBody::Bytes(b) => Full::new(b).map_err(|never| match never {}).boxed(),
            ReqBody::Stream { factory, .. } => factory(),
        }
    }
}

/// A boxed `HttpBody<Data = Bytes, Error = Error>` used as the response body
/// type after decompression / chunked decoding
pub type RespBody = http_body_util::combinators::BoxBody<Bytes, Error>;
