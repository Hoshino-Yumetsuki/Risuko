//! Body decompression for `Content-Encoding: gzip|br|deflate`

use std::io;

use async_compression::tokio::bufread::{BrotliDecoder, DeflateDecoder, GzipDecoder, ZlibDecoder};
use futures_util::TryStreamExt;
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
    // Only build the StreamReader/decoder pipeline when we actually need to
    // decode, so the no-op path doesn't pay for a Stream -> Reader -> Stream
    // round-trip and an extra allocation per chunk
    match enc.as_deref() {
        Some("gzip" | "x-gzip") => wrap(GzipDecoder::new(StreamReader::new(mapped))),
        Some("br") => wrap(BrotliDecoder::new(StreamReader::new(mapped))),
        // RFC 7230 §4.2.2 defines `deflate` as zlib-framed (RFC 1950)
        // wrapping raw DEFLATE (RFC 1951). Many servers historically sent
        // raw deflate too, so callers seeing decode failures from
        // non-compliant servers can disable auto-deflate. Treat the
        // commonly-seen `x-deflate` alias the same
        Some("deflate" | "x-deflate") => wrap(ZlibDecoder::new(StreamReader::new(mapped))),
        // Kept for callers that explicitly opt into raw DEFLATE handling.
        Some("raw-deflate") => wrap(DeflateDecoder::new(StreamReader::new(mapped))),
        _ => StreamBody::new(mapped).boxed(),
    }
}

fn wrap<D>(dec: D) -> RespBody
where
    D: tokio::io::AsyncRead + Send + Sync + Unpin + 'static,
{
    let s = ReaderStream::new(dec);
    StreamBody::new(s).boxed()
}

fn err_to_io(e: Error) -> io::Error {
    io::Error::other(e.to_string())
}
