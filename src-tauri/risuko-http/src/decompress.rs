//! Body decompression for `Content-Encoding: gzip|br|deflate`

use std::io;

use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder};
use futures_util::TryStreamExt;
use http_body::Frame;
use http_body_util::{BodyExt, StreamBody};
use tokio_util::io::{ReaderStream, StreamReader};

use crate::body::RespBody;
use crate::error::Error;

/// Wrap a hyper body so the indicated content encoding is decoded, returning the input body unchanged for unknown encodings
pub(crate) fn maybe_decompress(body: RespBody, encoding: Option<&str>) -> RespBody {
    let enc = encoding.map(|s| s.trim().to_ascii_lowercase());
    let mapped = body
        .into_data_stream()
        .map_err(err_to_io as fn(Error) -> io::Error);
    match enc.as_deref() {
        Some("gzip" | "x-gzip") => wrap(GzipDecoder::new(StreamReader::new(mapped))),
        Some("br") => wrap(BrotliDecoder::new(StreamReader::new(mapped))),
        // RFC 7230 §4.2.2 zlib-framed (RFC 1950) DEFLATE (RFC 1951); many servers sent raw deflate historically, and the commonly-seen `x-deflate` alias is treated the same
        Some("deflate" | "x-deflate") => wrap(ZlibDecoder::new(StreamReader::new(mapped))),
        _ => StreamBody::new(
            mapped
                .map_ok(Frame::data)
                .map_err(|e| Error::Body(e.to_string())),
        )
        .boxed(),
    }
}

fn wrap<D>(dec: D) -> RespBody
where
    D: tokio::io::AsyncRead + Send + Sync + Unpin + 'static,
{
    StreamBody::new(
        ReaderStream::new(dec)
            .map_ok(Frame::data)
            .map_err(|e| Error::Body(e.to_string())),
    )
    .boxed()
}

fn err_to_io(e: Error) -> io::Error {
    io::Error::other(e.to_string())
}
