use serde::{Deserialize, Serialize};
use std::fmt;

/// Risuko error codes for download failures
///
/// Codes are grouped by category:
/// - 1xx: General / unknown errors
/// - 2xx: Network errors (DNS, connection, timeout)
/// - 3xx: HTTP errors (4xx/5xx responses, redirects)
/// - 4xx: File system errors (disk full, permission, path)
/// - 5xx: Protocol-specific errors (torrent, ed2k, m3u8, ftp)
/// - 9xx: Internal / engine errors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorCode(pub u16);

impl ErrorCode {
    // -- General --
    pub const UNKNOWN: Self = Self(100);

    // -- Network --
    pub const DNS_RESOLUTION_FAILED: Self = Self(200);
    pub const CONNECTION_REFUSED: Self = Self(201);
    pub const CONNECTION_RESET: Self = Self(202);
    pub const CONNECTION_TIMEOUT: Self = Self(203);
    pub const NETWORK_UNREACHABLE: Self = Self(204);
    pub const TLS_HANDSHAKE_FAILED: Self = Self(205);
    pub const PROXY_CONNECTION_FAILED: Self = Self(206);

    // -- HTTP --
    pub const HTTP_UNAUTHORIZED: Self = Self(300);
    pub const HTTP_FORBIDDEN: Self = Self(301);
    pub const HTTP_NOT_FOUND: Self = Self(302);
    pub const HTTP_RANGE_NOT_SATISFIABLE: Self = Self(303);
    pub const HTTP_TOO_MANY_REQUESTS: Self = Self(304);
    pub const HTTP_SERVER_ERROR: Self = Self(305);
    pub const HTTP_SERVICE_UNAVAILABLE: Self = Self(306);
    pub const HTTP_REDIRECT_LOOP: Self = Self(307);
    pub const HTTP_RESPONSE_ERROR: Self = Self(308);
    /// Cloudflare bot-protection challenge detected on a 403/503/429
    /// response carrying cf-ray, `server: cloudflare`, or a "Just a
    /// moment..." body. Resolved by importing browser cookies plus a
    /// matching User-Agent
    pub const CLOUDFLARE_CHALLENGE: Self = Self(315);

    // -- File system --
    pub const DISK_FULL: Self = Self(400);
    pub const PERMISSION_DENIED: Self = Self(401);

    // -- Protocol-specific --
    pub const TORRENT_METADATA_FAILED: Self = Self(500);
    pub const TORRENT_NO_SEEDS: Self = Self(501);
    pub const TORRENT_INVALID_FILE: Self = Self(502);
    /// Pure-v2 magnet resolved the info dict but no peer served the BEP 52
    /// piece-layer hashes. The torrent is unverifiable until layers are
    /// obtained (importing the .torrent file always works)
    pub const TORRENT_PIECE_LAYERS_UNAVAILABLE: Self = Self(503);
    pub const ED2K_SERVER_UNREACHABLE: Self = Self(510);
    pub const ED2K_FILE_NOT_FOUND: Self = Self(511);
    pub const M3U8_PARSE_FAILED: Self = Self(520);
    pub const M3U8_SEGMENT_FAILED: Self = Self(521);
    pub const M3U8_DECRYPT_FAILED: Self = Self(522);
    pub const FTP_LOGIN_FAILED: Self = Self(530);
    pub const FTP_FILE_NOT_FOUND: Self = Self(531);
    pub const FTP_TRANSFER_FAILED: Self = Self(532);
    pub const SFTP_AUTH_FAILED: Self = Self(533);
    pub const SFTP_HOST_KEY_FAILED: Self = Self(534);
    pub const MEDIA_TOOL_NOT_FOUND: Self = Self(540);
    pub const MEDIA_AUTH_REQUIRED: Self = Self(541);
    pub const MEDIA_FORMAT_UNAVAILABLE: Self = Self(542);
    pub const USENET_AUTH_FAILED: Self = Self(550);
    pub const USENET_ARTICLE_UNAVAILABLE: Self = Self(551);
    pub const USENET_ARCHIVE_UNSAFE: Self = Self(552);
    pub const USENET_ARCHIVE_LIMIT: Self = Self(553);
    pub const USENET_REPAIR_FAILED: Self = Self(554);

    // -- Internal --
    pub const ENGINE_NOT_RUNNING: Self = Self(900);
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Returns true when `code` appears as a standalone ASCII number
///
/// Avoids `contains("500")` matching embedded runs like "5003" or "15000"
fn contains_status(haystack: &str, code: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(code) {
        let i = start + pos;
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
        let after = i + code.len();
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        start = i + 1;
    }
    false
}

/// Classify an error message string into an error code
///
/// For HTTP downloads, also pass the protocol kind for better classification
pub fn classify_error(msg: &str, protocol: &str) -> ErrorCode {
    let lower = msg.to_lowercase();

    // -- Network errors --
    if lower.contains("dns") || lower.contains("name resolution") || lower.contains("resolve host")
    {
        return ErrorCode::DNS_RESOLUTION_FAILED;
    }
    if lower.contains("connection refused") {
        return ErrorCode::CONNECTION_REFUSED;
    }
    if lower.contains("connection reset") || lower.contains("broken pipe") {
        return ErrorCode::CONNECTION_RESET;
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return ErrorCode::CONNECTION_TIMEOUT;
    }
    if lower.contains("network unreachable") || lower.contains("no route") {
        return ErrorCode::NETWORK_UNREACHABLE;
    }
    if lower.contains("tls") || lower.contains("ssl") || lower.contains("certificate") {
        return ErrorCode::TLS_HANDSHAKE_FAILED;
    }
    if lower.contains("proxy") {
        return ErrorCode::PROXY_CONNECTION_FAILED;
    }

    // -- HTTP status codes (skip for media: its match arm classifies these) --
    if protocol != "media" {
        // Cloudflare has to beat the generic 403/503/429 catch-alls
        // below. The HTTP downloader prefixes its message with
        // [cloudflare-challenge] when it sniffs the response (see
        // http.rs::looks_like_cloudflare_block)
        if lower.contains("[cloudflare-challenge]")
            || lower.contains("cloudflare challenge")
            || lower.contains("cf_clearance required")
        {
            return ErrorCode::CLOUDFLARE_CHALLENGE;
        }
        if contains_status(&lower, "401") || lower.contains("unauthorized") {
            return ErrorCode::HTTP_UNAUTHORIZED;
        }
        if contains_status(&lower, "403") || lower.contains("forbidden") {
            return ErrorCode::HTTP_FORBIDDEN;
        }
        if contains_status(&lower, "404") || lower.contains("not found") {
            // Distinguish HTTP 404 from file-not-found on disk
            if protocol == "http" || protocol == "m3u8" {
                return ErrorCode::HTTP_NOT_FOUND;
            }
        }
        if contains_status(&lower, "416") || lower.contains("range not satisfiable") {
            return ErrorCode::HTTP_RANGE_NOT_SATISFIABLE;
        }
        if contains_status(&lower, "429") || lower.contains("too many requests") {
            return ErrorCode::HTTP_TOO_MANY_REQUESTS;
        }
        if contains_status(&lower, "503") || lower.contains("service unavailable") {
            return ErrorCode::HTTP_SERVICE_UNAVAILABLE;
        }
        if lower.contains("5xx")
            || contains_status(&lower, "500")
            || contains_status(&lower, "502")
            || lower.contains("server error")
        {
            return ErrorCode::HTTP_SERVER_ERROR;
        }
        if lower.contains("redirect") {
            return ErrorCode::HTTP_REDIRECT_LOOP;
        }
    }

    // -- File system errors --
    if lower.contains("no space") || lower.contains("disk full") || lower.contains("quota") {
        return ErrorCode::DISK_FULL;
    }
    if lower.contains("permission denied") || lower.contains("access denied") {
        return ErrorCode::PERMISSION_DENIED;
    }

    // -- Protocol-specific patterns --
    match protocol {
        "torrent" => {
            // Check piece-layers FIRST: the message also contains the
            // word "metadata" / "peer" so a more general pattern would
            // shadow this typed error
            if lower.contains("piece layers unavailable") {
                return ErrorCode::TORRENT_PIECE_LAYERS_UNAVAILABLE;
            }
            if lower.contains("metadata") {
                return ErrorCode::TORRENT_METADATA_FAILED;
            }
            if lower.contains("no seeds") || lower.contains("no peers") {
                return ErrorCode::TORRENT_NO_SEEDS;
            }
            if lower.contains("invalid") || lower.contains("corrupt") {
                return ErrorCode::TORRENT_INVALID_FILE;
            }
        }
        "ed2k" => {
            if lower.contains("not found") {
                return ErrorCode::ED2K_FILE_NOT_FOUND;
            }
            if lower.contains("unreachable") || lower.contains("server") {
                return ErrorCode::ED2K_SERVER_UNREACHABLE;
            }
        }
        "m3u8" => {
            if lower.contains("parse") || lower.contains("playlist") {
                return ErrorCode::M3U8_PARSE_FAILED;
            }
            if lower.contains("decrypt") || lower.contains("aes") {
                return ErrorCode::M3U8_DECRYPT_FAILED;
            }
            if lower.contains("segment") {
                return ErrorCode::M3U8_SEGMENT_FAILED;
            }
        }
        "usenet" => {
            if lower.contains("auth") || lower.contains("credential") {
                return ErrorCode::USENET_AUTH_FAILED;
            }
            if lower.contains("archive safety:") {
                if lower.contains("unsafepath") {
                    return ErrorCode::USENET_ARCHIVE_UNSAFE;
                }
                if [
                    "entrycount",
                    "expandedbytes",
                    "entrybytes",
                    "nestingdepth",
                    "compressionratio",
                    "freespacereserve",
                    "activetime",
                ]
                .iter()
                .any(|variant| lower.contains(variant))
                {
                    return ErrorCode::USENET_ARCHIVE_LIMIT;
                }
            }
            if lower.contains("unsafe archive")
                || lower.contains("unsafe path")
                || lower.contains("unsafe par2")
            {
                return ErrorCode::USENET_ARCHIVE_UNSAFE;
            }
            if lower.contains("archive limit")
                || lower.contains("archive bomb")
                || lower.contains("par2 safety limit")
            {
                return ErrorCode::USENET_ARCHIVE_LIMIT;
            }
            if lower.contains("par2") || lower.contains("repair") {
                return ErrorCode::USENET_REPAIR_FAILED;
            }
            if lower.contains("article") || lower.contains("message id") {
                return ErrorCode::USENET_ARTICLE_UNAVAILABLE;
            }
        }
        "ftp" | "sftp" => {
            if lower.contains("login") || contains_status(&lower, "530") {
                return ErrorCode::FTP_LOGIN_FAILED;
            }
            if lower.contains("auth") {
                return ErrorCode::SFTP_AUTH_FAILED;
            }
            if lower.contains("host key") {
                return ErrorCode::SFTP_HOST_KEY_FAILED;
            }
            if lower.contains("not found") || contains_status(&lower, "550") {
                return ErrorCode::FTP_FILE_NOT_FOUND;
            }
            if lower.contains("transfer") {
                return ErrorCode::FTP_TRANSFER_FAILED;
            }
        }
        "media" => {
            if lower.contains("yt-dlp is not available") || lower.contains("command not found") {
                return ErrorCode::MEDIA_TOOL_NOT_FOUND;
            }
            if lower.contains("sign in")
                || lower.contains("login")
                || lower.contains("authentication")
                || lower.contains("members-only")
                || lower.contains("age-restricted")
            {
                return ErrorCode::MEDIA_AUTH_REQUIRED;
            }
            if lower.contains("requested format") || lower.contains("format is not available") {
                return ErrorCode::MEDIA_FORMAT_UNAVAILABLE;
            }
            if lower.contains("http error 404")
                || (contains_status(&lower, "404") && lower.contains("http"))
            {
                return ErrorCode::HTTP_NOT_FOUND;
            }
        }
        _ => {}
    }

    // -- Fallback for HTTP response errors --
    if (protocol == "http" || protocol == "m3u8")
        && (lower.contains("status") || lower.contains("http"))
    {
        return ErrorCode::HTTP_RESPONSE_ERROR;
    }

    ErrorCode::UNKNOWN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_dns_error() {
        assert_eq!(
            classify_error("dns error: failed to lookup address", "http"),
            ErrorCode::DNS_RESOLUTION_FAILED
        );
    }

    #[test]
    fn classify_timeout() {
        assert_eq!(
            classify_error("operation timed out", "http"),
            ErrorCode::CONNECTION_TIMEOUT
        );
    }

    #[test]
    fn classify_http_404() {
        assert_eq!(
            classify_error("HTTP 404 Not Found", "http"),
            ErrorCode::HTTP_NOT_FOUND
        );
    }

    #[test]
    fn classify_cloudflare_marker_beats_403() {
        assert_eq!(
            classify_error("[cloudflare-challenge] host=example.com status=403", "http"),
            ErrorCode::CLOUDFLARE_CHALLENGE
        );
    }

    #[test]
    fn classify_ftp_login() {
        assert_eq!(
            classify_error("530 Login incorrect", "ftp"),
            ErrorCode::FTP_LOGIN_FAILED
        );
    }

    #[test]
    fn classify_m3u8_decrypt() {
        assert_eq!(
            classify_error("failed to decrypt AES segment", "m3u8"),
            ErrorCode::M3U8_DECRYPT_FAILED
        );
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(
            classify_error("something weird happened", "http"),
            ErrorCode::UNKNOWN
        );
    }

    #[test]
    fn classify_torrent_resolve_not_dns() {
        // "Failed to resolve magnet: ..." must NOT be classified as DNS error
        assert_eq!(
            classify_error("Failed to resolve magnet: connection refused", "torrent"),
            ErrorCode::CONNECTION_REFUSED
        );
        assert_eq!(
            classify_error("Failed to resolve magnet metadata", "torrent"),
            ErrorCode::TORRENT_METADATA_FAILED
        );
    }

    #[test]
    fn classify_torrent_piece_layers_unavailable() {
        // The bt::magnet error contract: messages prefixed with this
        // string must surface as the typed code so the UI can prompt the
        // user to import the .torrent instead
        assert_eq!(
            classify_error(
                "Failed to resolve magnet: piece layers unavailable: no peer served the BEP 52 piece-layer hashes for this magnet",
                "torrent"
            ),
            ErrorCode::TORRENT_PIECE_LAYERS_UNAVAILABLE
        );
    }

    #[test]
    fn classify_fallback_http_only() {
        // "status" / "http" fallback should only fire for HTTP-like protocols
        assert_eq!(
            classify_error("HTTP tracker returned bad status", "http"),
            ErrorCode::HTTP_RESPONSE_ERROR
        );
        assert_eq!(
            classify_error("HTTP tracker returned bad status", "torrent"),
            ErrorCode::UNKNOWN
        );
    }
    #[test]
    fn classify_media_tool_not_found() {
        assert_eq!(
            classify_error("yt-dlp is not available in PATH", "media"),
            ErrorCode::MEDIA_TOOL_NOT_FOUND
        );
        assert_eq!(
            classify_error("yt-dlp: command not found", "media"),
            ErrorCode::MEDIA_TOOL_NOT_FOUND
        );
    }

    #[test]
    fn classify_media_auth_required() {
        for msg in &[
            "This video requires you to sign in",
            "Please login to view this content",
            "authentication required",
            "This is a members-only video",
            "This content is age-restricted",
        ] {
            assert_eq!(
                classify_error(msg, "media"),
                ErrorCode::MEDIA_AUTH_REQUIRED,
                "expected MEDIA_AUTH_REQUIRED for: {msg}"
            );
        }
    }

    #[test]
    fn classify_media_format_unavailable() {
        assert_eq!(
            classify_error("ERROR: requested format is not available", "media"),
            ErrorCode::MEDIA_FORMAT_UNAVAILABLE
        );
        assert_eq!(
            classify_error("the format is not available for this video", "media"),
            ErrorCode::MEDIA_FORMAT_UNAVAILABLE
        );
    }

    #[test]
    fn classify_status_code_respects_digit_boundaries() {
        // A digit run that embeds "500" must not classify as HTTP 5xx
        assert_eq!(
            classify_error("received 5003 bytes then the stream ended", "http"),
            ErrorCode::UNKNOWN
        );
        // A standalone 500 status still classifies correctly
        assert_eq!(
            classify_error("HTTP 500 returned by origin", "http"),
            ErrorCode::HTTP_SERVER_ERROR
        );
    }

    #[test]
    fn classify_media_wins_over_http_403() {
        // A YouTube-specific message that also contains an HTTP status code
        // must be classified by the media arm, not the generic HTTP arm
        assert_eq!(
            classify_error("HTTP Error 403: age-restricted content", "media"),
            ErrorCode::MEDIA_AUTH_REQUIRED
        );
    }

    #[test]
    fn classifies_par2_safety_failures_as_usenet_archive_failures() {
        assert_eq!(
            classify_error("PAR2 safety limit: too many recovery blocks", "usenet"),
            ErrorCode::USENET_ARCHIVE_LIMIT
        );
        assert_eq!(
            classify_error("unsafe PAR2 filename: ../escape.bin", "usenet"),
            ErrorCode::USENET_ARCHIVE_UNSAFE
        );
    }

    #[test]
    fn classifies_formatted_archive_pipeline_safety_failures() {
        use crate::engine::archive_pipeline::ArchivePipelineError;
        use crate::engine::archive_safety::ArchiveSafetyError;

        let unsafe_path = ArchivePipelineError::Safety(ArchiveSafetyError::UnsafePath).to_string();
        assert_eq!(
            classify_error(&unsafe_path, "usenet"),
            ErrorCode::USENET_ARCHIVE_UNSAFE
        );

        let expanded = ArchivePipelineError::Safety(ArchiveSafetyError::ExpandedBytes).to_string();
        assert_eq!(
            classify_error(&expanded, "usenet"),
            ErrorCode::USENET_ARCHIVE_LIMIT
        );
    }
}
