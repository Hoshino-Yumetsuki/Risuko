use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use http::{HeaderMap, StatusCode, Version};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use url::Url;

use crate::body::{BodyStream, RespBody};
use crate::error::{Error, Result};

pub struct Response {
    status: StatusCode,
    version: Version,
    headers: HeaderMap,
    url: Url,
    body: Option<RespBody>,
}

impl Response {
    pub(crate) fn new(
        status: StatusCode,
        version: Version,
        headers: HeaderMap,
        url: Url,
        body: RespBody,
    ) -> Self {
        Self {
            status,
            version,
            headers,
            url,
            body: Some(body),
        }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn content_length(&self) -> Option<u64> {
        self.headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
    }

    pub fn error_for_status(self) -> Result<Self> {
        if self.status.is_client_error() || self.status.is_server_error() {
            Err(Error::Status(self.status))
        } else {
            Ok(self)
        }
    }

    pub fn error_for_status_ref(&self) -> Result<&Self> {
        if self.status.is_client_error() || self.status.is_server_error() {
            Err(Error::Status(self.status))
        } else {
            Ok(self)
        }
    }

    pub async fn bytes(mut self) -> Result<Bytes> {
        let body = self
            .body
            .take()
            .ok_or_else(|| Error::Body("body already consumed".into()))?;
        let collected = body
            .collect()
            .await
            .map_err(|e| Error::Body(e.to_string()))?;
        Ok(collected.to_bytes())
    }

    pub(crate) async fn drain(mut self) -> Result<()> {
        if let Some(body) = self.body.take() {
            let mut stream = BodyStream::new(body);
            while let Some(chunk) = stream.next().await {
                chunk?;
            }
        }
        Ok(())
    }

    pub async fn text(self) -> Result<String> {
        let bytes = self.bytes().await?;
        String::from_utf8(bytes.into()).map_err(|e| Error::Decode(e.to_string()))
    }

    pub async fn json<T: DeserializeOwned>(self) -> Result<T> {
        let bytes = self.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }

    pub fn bytes_stream(mut self) -> impl Stream<Item = Result<Bytes>> + Send + 'static {
        let body = self.body.take().expect("body already consumed");
        BodyStream::new(body)
    }
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("version", &self.version)
            .field("url", &self.url.as_str())
            .finish()
    }
}
