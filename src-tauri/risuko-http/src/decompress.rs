//! Body decompression for `Content-Encoding: gzip|br|deflate`

use std::io;

use async_compression::tokio::bufread::{BrotliDecoder, DeflateDecoder, GzipDecoder};
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::BodyExt;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::body::{BodyStream, RespBody, StreamBody};
use crate::error::Error;

/// Wrap a hyper body so that the indicated content encoding is transparently
/// decoded. Returns the input body unchanged for unknown encodings
pub(crate) fn maybe_decompress(body: RespBody, encoding: Option<&str>) -> RespBody {
    let enc = encoding.map(|s| s.trim().to_ascii_lowercase());
    let stream = BodyStream::new(body);
    let mapped = stream.map_err(err_to_io as fn(Error) -> io::Error);
    let reader = StreamReader::new(mapped);
    match enc.as_deref() {
        Some("gzip" | "x-gzip") => wrap(GzipDecoder::new(reader)),
        Some("br") => wrap(BrotliDecoder::new(reader)),
        Some("deflate") => wrap(DeflateDecoder::new(reader)),
        _ => {
            // Caller already chose not to decompress. Re-wrap untouched
            let s = ReaderStream::new(reader).map(|r| r.map(Bytes::from));
            StreamBody::new(s).boxed()
        }
    }
}

fn wrap<D>(dec: D) -> RespBody
where
    D: tokio::io::AsyncRead + Send + Sync + Unpin + 'static,
{
    let s = ReaderStream::new(dec).map(|r| r.map(Bytes::from));
    StreamBody::new(s).boxed()
}

fn err_to_io(e: Error) -> io::Error {
    io::Error::other(e.to_string())
}
