//! S3 (and S3-compatible) upload sink — works with AWS S3, MinIO,
//! Backblaze B2, Cloudflare R2, Wasabi, Garage, etc
//!
//! Uses SigV4 single-PUT with `UNSIGNED-PAYLOAD` so we can stream the file
//! body without a pre-pass to compute its SHA-256.

//! TODO - Multipart upload
//! Next step for >5 GB objects

use std::time::Duration;

use async_trait::async_trait;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use risuko_http::{Client, ClientBuilder, Url};
use sha2::{Digest, Sha256};

use super::sink::{S3Config, UploadControl, UploadFile, UploadSink};

type HmacSha256 = Hmac<Sha256>;

const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

pub struct S3Sink {
    cfg: S3Config,
    client: Client,
    base_url: Url,
    /// Resolved host header used in canonical request — always
    /// `host[:port]` of the endpoint, never the bucket-prefixed form
    host_header: String,
}

impl S3Sink {
    pub fn new(cfg: S3Config) -> Result<Self, String> {
        let endpoint = cfg.endpoint.trim_end_matches('/');
        if endpoint.is_empty() {
            return Err("S3 endpoint is empty".into());
        }
        if cfg.bucket.trim().is_empty() {
            return Err("S3 bucket is empty".into());
        }
        if cfg.access_key_id.trim().is_empty() {
            return Err("S3 access key is empty".into());
        }
        if cfg.secret_access_key.trim().is_empty() {
            return Err("S3 secret access key is empty".into());
        }

        let base_url = Url::parse(endpoint).map_err(|e| format!("Invalid S3 endpoint URL: {e}"))?;
        let host_header = match base_url.port() {
            Some(p) => format!(
                "{}:{}",
                base_url
                    .host_str()
                    .ok_or_else(|| "S3 endpoint missing host".to_string())?,
                p
            ),
            None => base_url
                .host_str()
                .ok_or_else(|| "S3 endpoint missing host".to_string())?
                .to_string(),
        };

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(30 * 60))
            .connect_timeout(Duration::from_secs(30))
            .user_agent("risuko/upload")
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

        Ok(Self {
            cfg,
            client,
            base_url,
            host_header,
        })
    }

    /// Build the object key from the prefix + remote-relative path
    /// Both segments are joined with `/` and any leading `/` is stripped
    fn object_key(&self, remote_relative: &str) -> String {
        let pre = self.cfg.prefix.trim_matches('/');
        let rel = remote_relative.trim_start_matches('/');
        if pre.is_empty() {
            rel.to_string()
        } else {
            format!("{pre}/{rel}")
        }
    }

    /// Build the absolute URL for an object. Path-style:
    /// `{endpoint}/{bucket}/{key}`. Virtual-host style:
    /// `{scheme}://{bucket}.{host}/{key}`
    fn object_url(&self, key: &str) -> Result<Url, String> {
        let encoded = uri_encode(key, false);
        if self.cfg.force_path_style {
            let mut u = self.base_url.clone();
            let path = format!(
                "{}/{}/{}",
                u.path().trim_end_matches('/'),
                uri_encode(&self.cfg.bucket, false),
                encoded
            );
            u.set_path(&path);
            Ok(u)
        } else {
            let host = self
                .base_url
                .host_str()
                .ok_or_else(|| "S3 endpoint missing host".to_string())?;
            let scheme = self.base_url.scheme();
            let port_part = match self.base_url.port() {
                Some(p) => format!(":{p}"),
                None => String::new(),
            };
            let s = format!(
                "{scheme}://{}.{host}{port_part}/{encoded}",
                uri_encode(&self.cfg.bucket, false)
            );
            Url::parse(&s).map_err(|e| format!("Invalid object URL: {e}"))
        }
    }

    /// Canonical host header for the request. For virtual-host style this
    /// becomes `{bucket}.{host[:port]}` so the signature matches what the
    /// server sees in its `Host` header
    fn canonical_host(&self) -> String {
        if self.cfg.force_path_style {
            self.host_header.clone()
        } else {
            format!("{}.{}", self.cfg.bucket, self.host_header)
        }
    }

    /// Compute SigV4 signature for a PUT with unsigned payload
    /// Returns the full `Authorization` header value
    fn sign_put(&self, url: &Url, amz_date: &str, datestamp: &str) -> String {
        let region = if self.cfg.region.trim().is_empty() {
            "us-east-1"
        } else {
            self.cfg.region.trim()
        };
        let service = "s3";
        let host = self.canonical_host();

        // Canonical URI: the path component, with each segment URI-encoded
        // S3 SigV4 wants `/` to remain unencoded, only the segment chars
        let path = url.path();
        let canonical_uri = canonical_uri(path);
        // Empty query for plain PUT
        let canonical_query = String::new();
        // Headers must be lowercase, sorted, trimmed
        let canonical_headers =
            format!("host:{host}\nx-amz-content-sha256:{UNSIGNED}\nx-amz-date:{amz_date}\n");
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "PUT\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{UNSIGNED}"
        );

        let hashed_request = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let scope = format!("{datestamp}/{region}/{service}/aws4_request");
        let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashed_request}");

        let signing_key =
            derive_signing_key(&self.cfg.secret_access_key, datestamp, region, service);
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
            self.cfg.access_key_id
        )
    }
}

#[async_trait]
impl UploadSink for S3Sink {
    async fn upload(&self, file: &UploadFile, ctl: &UploadControl) -> Result<String, String> {
        if ctl.cancel.is_cancelled() {
            return Err("cancelled".into());
        }

        // Single PUT tops out at 5 GiB per the S3 API contract — fail fast
        // here so the user gets a clear message instead of an obscure error
        // mid-stream after re-uploading megabytes. Multipart upload is
        // tracked by the file-level TODO above
        const SINGLE_PUT_MAX: u64 = 5 * 1024 * 1024 * 1024;
        if file.size > SINGLE_PUT_MAX {
            return Err("file too large for single PUT (over 5 GiB); multipart upload not yet implemented".into());
        }

        let key = self.object_key(&file.remote_relative);
        let url = self.object_url(&key)?;

        let now = chrono_now_utc();
        let amz_date = now.0;
        let datestamp = now.1;

        let auth = self.sign_put(&url, &amz_date, &datestamp);
        // Wrap the file stream so each yielded chunk reports progress back
        // through `ctl` and observes cancellation mid-stream
        let total = file.size;
        let progress = ctl.clone();
        let body = risuko_http::file_stream_body_with_progress(
            file.local_path.clone(),
            total,
            move |sent| progress.report(sent.min(total), total),
            Some(ctl.cancel.clone()),
        );

        let req = self
            .client
            .put(url.as_str())
            .stream_body(body)
            .header("host", self.canonical_host())
            .header("x-amz-content-sha256", UNSIGNED)
            .header("x-amz-date", amz_date)
            .header("authorization", auth)
            .header("content-length", file.size.to_string());

        let send_fut = req.send();
        let resp = tokio::select! {
            _ = ctl.cancel.cancelled() => return Err("cancelled".into()),
            r = send_fut => r.map_err(|e| format!("PUT failed: {e}"))?,
        };

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("S3 PUT {url} returned {status}: {body}"));
        }

        // Final pin so the UI sees 100% even if the last chunk's report was
        // raced by the response arriving
        ctl.report(file.size, file.size);
        Ok(url.to_string())
    }

    async fn test(&self) -> Result<(), String> {
        // HEAD on the bucket root (path-style: /bucket; vhost: /)
        // 200/403 means reachable; 404 means missing
        let url = if self.cfg.force_path_style {
            let mut u = self.base_url.clone();
            let path = format!(
                "{}/{}",
                u.path().trim_end_matches('/'),
                uri_encode(&self.cfg.bucket, false)
            );
            u.set_path(&path);
            u
        } else {
            self.object_url("")?
        };

        let now = chrono_now_utc();
        let amz_date = now.0;
        let datestamp = now.1;
        let region = if self.cfg.region.trim().is_empty() {
            "us-east-1"
        } else {
            self.cfg.region.trim()
        };
        let host = self.canonical_host();
        let canonical_uri = canonical_uri(url.path());
        let canonical_headers =
            format!("host:{host}\nx-amz-content-sha256:{UNSIGNED}\nx-amz-date:{amz_date}\n");
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request =
            format!("HEAD\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{UNSIGNED}");
        let hashed_request = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let scope = format!("{datestamp}/{region}/s3/aws4_request");
        let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashed_request}");
        let signing_key = derive_signing_key(&self.cfg.secret_access_key, &datestamp, region, "s3");
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));
        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
            self.cfg.access_key_id
        );

        let resp = self
            .client
            .request(risuko_http::Method::HEAD, url.as_str())
            .header("host", host)
            .header("x-amz-content-sha256", UNSIGNED)
            .header("x-amz-date", amz_date)
            .header("authorization", auth)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("HEAD bucket: {e}"))?;

        let status = resp.status();
        // 200 = bucket reachable + ListBucket permission. 403 = bucket
        // exists and our credentials are recognised but lack ListBucket;
        // AWS HeadBucket returns *no body* for 403, so we cannot inspect an
        // error code here — treat 403 as a successful reachability check so
        // upload-only keys (the common case for app-managed buckets) pass.
        // Authentication failures surface as 400 ("InvalidAccessKeyId",
        // "SignatureDoesNotMatch") rather than 403, so accepting 403 here
        // does not mask credential errors
        if status.is_success() || status.as_u16() == 403 {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(format!("S3 HEAD bucket returned {status}: {body}"))
    }
}

// -- crypto helpers --

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn derive_signing_key(secret: &str, datestamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), datestamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// AWS-flavoured URI encoding. `encode_slash=false` keeps `/` as-is (path
/// segments). Per SigV4 spec
fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b'/' if !encode_slash => out.push('/'),
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        // Re-encode while preserving `/`
        let decoded = percent_encoding::percent_decode_str(path)
            .decode_utf8_lossy()
            .into_owned();
        uri_encode(&decoded, false)
    }
}

/// Returns (`yyyymmddThhmmssZ`, `yyyymmdd`) for the current UTC time
fn chrono_now_utc() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_ymdhms(secs);
    let amz = format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z");
    let day = format!("{y:04}{mo:02}{d:02}");
    (amz, day)
}

/// Convert UNIX epoch seconds (UTC) to (year, month, day, hour, min, sec)
/// Algorithm from Howard Hinnant's `civil_from_days`
fn epoch_to_ymdhms(secs: u64) -> (i64, u8, u8, u8, u8, u8) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let h = (rem / 3600) as u8;
    let mi = ((rem % 3600) / 60) as u8;
    let s = (rem % 60) as u8;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let mo = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(endpoint: &str, bucket: &str, prefix: &str, path_style: bool) -> S3Config {
        S3Config {
            endpoint: endpoint.into(),
            region: "us-east-1".into(),
            bucket: bucket.into(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            prefix: prefix.into(),
            force_path_style: path_style,
        }
    }

    // -- constructor validation --

    #[test]
    fn rejects_empty_endpoint() {
        let mut c = cfg("", "bkt", "", true);
        c.endpoint = String::new();
        assert!(S3Sink::new(c).is_err());
    }

    #[test]
    fn rejects_empty_bucket() {
        let c = cfg("https://s3.amazonaws.com", "", "", false);
        assert!(S3Sink::new(c).is_err());
    }

    #[test]
    fn rejects_empty_access_key() {
        let mut c = cfg("https://s3.amazonaws.com", "bkt", "", false);
        c.access_key_id = String::new();
        assert!(S3Sink::new(c).is_err());
    }

    #[test]
    fn rejects_malformed_endpoint() {
        let c = cfg("not a url", "bkt", "", false);
        assert!(S3Sink::new(c).is_err());
    }

    // -- object_key --

    #[test]
    fn object_key_no_prefix() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "b", "", false)).unwrap();
        assert_eq!(s.object_key("file.bin"), "file.bin");
        assert_eq!(s.object_key("/file.bin"), "file.bin");
        assert_eq!(s.object_key("dir/file.bin"), "dir/file.bin");
    }

    #[test]
    fn object_key_with_prefix() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "b", "uploads", false)).unwrap();
        assert_eq!(s.object_key("file.bin"), "uploads/file.bin");
        assert_eq!(s.object_key("dir/file.bin"), "uploads/dir/file.bin");
    }

    #[test]
    fn object_key_strips_prefix_slashes() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "b", "/uploads/", false)).unwrap();
        assert_eq!(s.object_key("file.bin"), "uploads/file.bin");
    }

    // -- object_url --

    #[test]
    fn object_url_path_style() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "mybucket", "", true)).unwrap();
        let u = s.object_url("foo/bar.bin").unwrap();
        assert_eq!(u.as_str(), "https://s3.amazonaws.com/mybucket/foo/bar.bin");
    }

    #[test]
    fn object_url_vhost_style() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "mybucket", "", false)).unwrap();
        let u = s.object_url("foo/bar.bin").unwrap();
        assert_eq!(u.as_str(), "https://mybucket.s3.amazonaws.com/foo/bar.bin");
    }

    #[test]
    fn object_url_path_style_minio_with_port() {
        let s = S3Sink::new(cfg("http://minio.local:9000", "data", "", true)).unwrap();
        let u = s.object_url("a.bin").unwrap();
        assert_eq!(u.as_str(), "http://minio.local:9000/data/a.bin");
    }

    #[test]
    fn object_url_vhost_with_port() {
        let s = S3Sink::new(cfg("http://s3.local:9000", "bkt", "", false)).unwrap();
        let u = s.object_url("a.bin").unwrap();
        assert_eq!(u.as_str(), "http://bkt.s3.local:9000/a.bin");
    }

    #[test]
    fn object_url_encodes_special_chars() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "b", "", true)).unwrap();
        let u = s.object_url("hello world.bin").unwrap();
        assert!(u.as_str().ends_with("/b/hello%20world.bin"), "got {u}");
    }

    // -- canonical_host --

    #[test]
    fn canonical_host_path_style_strips_bucket() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "mybucket", "", true)).unwrap();
        assert_eq!(s.canonical_host(), "s3.amazonaws.com");
    }

    #[test]
    fn canonical_host_vhost_prefixes_bucket() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "mybucket", "", false)).unwrap();
        assert_eq!(s.canonical_host(), "mybucket.s3.amazonaws.com");
    }

    #[test]
    fn canonical_host_includes_port() {
        let s = S3Sink::new(cfg("http://minio.local:9000", "bkt", "", true)).unwrap();
        assert_eq!(s.canonical_host(), "minio.local:9000");
    }

    // -- pure helpers --

    #[test]
    fn epoch_basic() {
        // 2024-01-02T03:04:05Z = 1704164645
        let t = epoch_to_ymdhms(1_704_164_645);
        assert_eq!(t, (2024, 1, 2, 3, 4, 5));
    }

    #[test]
    fn epoch_unix_zero() {
        assert_eq!(epoch_to_ymdhms(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_leap_year_feb_29() {
        // 2024-02-29T00:00:00Z = 1709164800
        assert_eq!(epoch_to_ymdhms(1_709_164_800), (2024, 2, 29, 0, 0, 0));
    }

    #[test]
    fn epoch_y2k_boundary() {
        // 2000-03-01T00:00:00Z = 951868800 (after century leap year)
        assert_eq!(epoch_to_ymdhms(951_868_800), (2000, 3, 1, 0, 0, 0));
    }

    #[test]
    fn uri_encode_basic() {
        assert_eq!(uri_encode("hello world", false), "hello%20world");
        assert_eq!(uri_encode("a/b c", false), "a/b%20c");
        assert_eq!(uri_encode("a/b c", true), "a%2Fb%20c");
    }

    #[test]
    fn uri_encode_preserves_unreserved() {
        // RFC 3986 unreserved set: ALPHA / DIGIT / "-" / "." / "_" / "~"
        assert_eq!(uri_encode("Az09-._~", false), "Az09-._~");
    }

    #[test]
    fn uri_encode_uppercases_hex() {
        // SigV4 mandates uppercase hex in percent-encoded triplets
        assert_eq!(uri_encode("\n", false), "%0A");
        assert_eq!(uri_encode("\x7f", false), "%7F");
    }

    #[test]
    fn canonical_uri_empty_becomes_root() {
        assert_eq!(canonical_uri(""), "/");
    }

    #[test]
    fn canonical_uri_re_encodes() {
        // Already-encoded input should round-trip through decode + re-encode
        // and produce the same canonical form
        assert_eq!(canonical_uri("/a/b%20c"), "/a/b%20c");
        assert_eq!(canonical_uri("/a/b c"), "/a/b%20c");
    }

    // -- crypto --

    #[test]
    fn signing_key_matches_aws_example() {
        // From AWS docs: signing-key derivation example
        let key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
            "iam",
        );
        let expected = "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d";
        assert_eq!(hex::encode(&key), expected);
    }

    #[test]
    fn hmac_sha256_known_vector() {
        // RFC 4231 test case 1
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex::encode(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    // -- sign_put --

    #[test]
    fn sign_put_is_deterministic_for_fixed_inputs() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "bkt", "", true)).unwrap();
        let url = s.object_url("file.bin").unwrap();
        let a = s.sign_put(&url, "20240101T000000Z", "20240101");
        let b = s.sign_put(&url, "20240101T000000Z", "20240101");
        assert_eq!(a, b, "signing must be deterministic");
        assert!(a.starts_with("AWS4-HMAC-SHA256 "));
        assert!(a.contains("Credential=AKIAIOSFODNN7EXAMPLE/20240101/us-east-1/s3/aws4_request"));
        assert!(a.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert!(a.contains("Signature="));
    }

    #[test]
    fn sign_put_different_dates_produce_different_signatures() {
        let s = S3Sink::new(cfg("https://s3.amazonaws.com", "bkt", "", true)).unwrap();
        let url = s.object_url("file.bin").unwrap();
        let a = s.sign_put(&url, "20240101T000000Z", "20240101");
        let b = s.sign_put(&url, "20240102T000000Z", "20240102");
        assert_ne!(a, b);
    }

    #[test]
    fn sign_put_falls_back_to_us_east_1_when_region_blank() {
        let mut c = cfg("https://s3.amazonaws.com", "bkt", "", true);
        c.region = "   ".into();
        let s = S3Sink::new(c).unwrap();
        let url = s.object_url("file.bin").unwrap();
        let auth = s.sign_put(&url, "20240101T000000Z", "20240101");
        assert!(auth.contains("/us-east-1/s3/"));
    }
}
