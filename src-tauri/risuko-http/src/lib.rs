//! HTTP client used by Risuko

mod body;
mod client;
mod connector;
mod cookies;
mod decompress;
pub mod doh;
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
pub use doh::{DohConfig, DohResolver};
pub use error::{Error, Result};
pub use into_url::IntoUrl;
pub use proxy::Proxy;
pub use redirect::Policy;
pub use request::RequestBuilder;
pub use resolver::{set_global_resolver, Addrs, GaiResolver, Resolve, Resolving};
pub use response::Response;

pub use http::header;
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
pub use url::Url;

/// Build a streaming `ReqBody` that reads from a fresh `tokio::fs::File` each
/// time it is polled. For large file uploads (WebDAV PUT, S3 PUT)
///
/// The path is captured by the factory; every send (including redirect/retry
/// replays) opens a new `File` so the stream re-drives from byte zero. Calls
/// `on_progress(bytes_sent_so_far)` after each chunk, aborts mid-stream when
/// `cancel` is set
///
/// The progress total is per send attempt (counters reset on retry/redirect,
/// so callers should clamp it against the known total). Cancel is checked
/// before every chunk yield, so even multi-GB uploads interrupt promptly
///
/// `_content_length` is accepted for source-compat but ignored — the advertised
/// `Content-Length` comes from the file's actual size at construction. Trusting
/// caller-supplied lengths risks stale framing and a truncated upload
pub fn file_stream_body_with_progress<F>(
    path: std::path::PathBuf,
    _content_length: u64,
    on_progress: F,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> ReqBody
where
    F: Fn(u64) + Send + Sync + 'static,
{
    file_stream_body_range_inner(
        path,
        0,
        None,
        Some(std::sync::Arc::new(on_progress)),
        cancel,
    )
}

/// Same as [`file_stream_body_with_progress`] but reads only `len` bytes
/// starting at byte `offset`. Used for S3 multipart UploadPart so each part
/// streams its own slice of the source file without buffering it in memory
pub fn file_stream_body_range_with_progress<F>(
    path: std::path::PathBuf,
    offset: u64,
    len: u64,
    on_progress: F,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> ReqBody
where
    F: Fn(u64) + Send + Sync + 'static,
{
    file_stream_body_range_inner(
        path,
        offset,
        Some(len),
        Some(std::sync::Arc::new(on_progress)),
        cancel,
    )
}

fn file_stream_body_range_inner(
    path: std::path::PathBuf,
    offset: u64,
    content_length: Option<u64>,
    on_progress: Option<std::sync::Arc<dyn Fn(u64) + Send + Sync + 'static>>,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> ReqBody {
    use futures_util::stream::TryStreamExt;
    use http_body::Frame;
    use http_body_util::{combinators::BoxBody, StreamBody};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    // Stat the file at construction so the advertised `Content-Length` matches
    // what the body delivers. Doing it only inside the lazy stream factory was
    // too late — the request layer had already framed headers from the
    // (possibly stale) caller-supplied length, so a shrunk file or over-large
    // `take` produced framing that disagreed with the body
    //
    // If the stat fails (file missing, permission denied) fall back to the
    // caller-supplied length so the deferred open in the stream factory still
    // surfaces the IO error; the request fails anyway, one layer deeper
    let verified_length = match std::fs::metadata(&path) {
        Ok(meta) => {
            let remaining = meta.len().saturating_sub(offset);
            Some(match content_length {
                Some(n) => n.min(remaining),
                None => remaining,
            })
        }
        Err(_) => content_length,
    };
    let take = verified_length;

    ReqBody::from_stream(
        move || {
            let path = path.clone();
            let on_progress = on_progress.clone();
            let cancel = cancel.clone();
            let counter = Arc::new(AtomicU64::new(0));
            // Lazily open the file the first time the body is polled. Any IO
            // error becomes a body error — hyper will surface it to the caller
            let stream = async_stream::try_stream! {
                use tokio::io::{AsyncReadExt, AsyncSeekExt};
                let mut file = tokio::fs::File::open(&path).await
                    .map_err(|e| Error::Body(format!("open {}: {e}", path.display())))?;
                // Defense-in-depth: re-validate the requested range against
                // the file's actual size at send time. If it shrank between
                // construction and now we'd otherwise hang the connection
                // after delivering a short body
                let file_size = file.metadata().await
                    .map_err(|e| Error::Body(format!("stat {}: {e}", path.display())))?
                    .len();
                let remaining = file_size.saturating_sub(offset);
                let effective_take = match take {
                    Some(n) if n > remaining => {
                        Err(Error::Body(format!(
                            "requested range {n} bytes from offset {offset} but only {remaining} available in {}",
                            path.display()
                        )))?;
                        unreachable!()
                    }
                    Some(n) => Some(n.min(remaining)),
                    None => None,
                };
                if offset > 0 {
                    file.seek(std::io::SeekFrom::Start(offset)).await
                        .map_err(|e| Error::Body(format!("seek {} to {offset}: {e}", path.display())))?;
                }
                let reader: Box<dyn tokio::io::AsyncRead + Send + Sync + Unpin> = match effective_take {
                    Some(n) => Box::new(file.take(n)),
                    None => Box::new(file),
                };
                let reader = tokio_util::io::ReaderStream::new(reader);
                let mut reader = std::pin::pin!(reader);
                use futures_util::StreamExt;
                while let Some(chunk) = reader.next().await {
                    if let Some(ref c) = cancel {
                        if c.is_cancelled() {
                            Err(Error::Body("cancelled".into()))?;
                        }
                    }
                    let bytes = chunk.map_err(|e| Error::Body(e.to_string()))?;
                    if let Some(ref cb) = on_progress {
                        let sent = counter.fetch_add(bytes.len() as u64, Ordering::Relaxed)
                            + bytes.len() as u64;
                        cb(sent);
                    }
                    yield bytes;
                }
            };
            let frame_stream = stream.map_ok(Frame::data);
            BoxBody::new(StreamBody::new(frame_stream))
        },
        verified_length,
    )
}
