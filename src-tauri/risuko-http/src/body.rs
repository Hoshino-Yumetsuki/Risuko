//! Request/response body types

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use pin_project_lite::pin_project;

use crate::error::Error;

/// Body type sent in outgoing requests. Always a single `Bytes` chunk
/// (Risuko never streams request bodies — at most a JSON value or a small
/// XML SOAP envelope)
#[derive(Clone, Debug)]
pub enum ReqBody {
    Empty,
    Bytes(Bytes),
}

impl ReqBody {
    pub fn from_bytes(b: impl Into<Bytes>) -> Self {
        ReqBody::Bytes(b.into())
    }

    pub(crate) fn into_hyper_body(self) -> BoxBody<Bytes, Error> {
        match self {
            ReqBody::Empty => http_body_util::Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
            ReqBody::Bytes(b) => Full::new(b).map_err(|never| match never {}).boxed(),
        }
    }
}

pin_project! {
    /// Wrapper that converts a hyper body into a `Stream<Item = Result<Bytes>>`
    pub struct BodyStream<B> {
        #[pin]
        inner: B,
    }
}

impl<B> BodyStream<B> {
    pub fn new(inner: B) -> Self {
        Self { inner }
    }
}

impl<B> futures_util::Stream for BodyStream<B>
where
    B: HttpBody<Data = Bytes>,
    B::Error: Into<Error>,
{
    type Item = Result<Bytes, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match futures_util::ready!(this.inner.as_mut().poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(b) => return Poll::Ready(Some(Ok(b))),
                    Err(_trailers) => continue,
                },
                Some(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                None => return Poll::Ready(None),
            }
        }
    }
}

/// A boxed `HttpBody<Data = Bytes, Error = Error>` used as the response body
/// type after decompression / chunked decoding
pub type RespBody = http_body_util::combinators::BoxBody<Bytes, Error>;

// Adapter that turns a `Stream<Item = io::Result<Bytes>>` (e.g. the output
// of `async-compression`) back into an `HttpBody`
pin_project! {
    pub struct StreamBody<S> {
        #[pin]
        stream: S,
    }
}

impl<S> StreamBody<S> {
    pub fn new(stream: S) -> Self {
        Self { stream }
    }
}

impl<S> HttpBody for StreamBody<S>
where
    S: futures_util::Stream<Item = std::io::Result<Bytes>>,
{
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        match futures_util::ready!(this.stream.poll_next(cx)) {
            Some(Ok(b)) => Poll::Ready(Some(Ok(Frame::data(b)))),
            Some(Err(e)) => Poll::Ready(Some(Err(Error::Body(e.to_string())))),
            None => Poll::Ready(None),
        }
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
