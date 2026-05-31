use serde::{Deserialize, Serialize};
use std::fmt;

/// Risuko error codes for download failures.
///
/// Codes are grouped by category:
/// - 1xx: General / unknown errors
/// - 2xx: Network errors (DNS, connection, timeout)
/// - 3xx: HTTP errors (4xx/5xx responses, redirects)
/// - 4xx: File system errors (disk full, permission, path)
/// - 5xx: Protocol-specific errors (torrent, ed2k, m3u8, ftp)
/// - 6xx: Resource errors (out of memory, too many connections)
/// - 9xx: Internal / engine errors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorCode(pub u16);

impl ErrorCode {
    // -- General --
    pub const UNKNOWN: Self = Self(100);
    pub const CANCELLED: Self = Self(101);
    pub const CHECKSUM_MISMATCH: Self = Self(102);

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
    pub const FILE_NOT_FOUND: Self = Self(402);
    pub const FILE_ALREADY_EXISTS: Self = Self(403);
    pub const INVALID_PATH: Self = Self(404);
    pub const IO_ERROR: Self = Self(405);

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

    // -- Resource --
    pub const OUT_OF_MEMORY: Self = Self(600);
    pub const TOO_MANY_CONNECTIONS: Self = Self(601);

    // -- Internal --
    pub const ENGINE_NOT_RUNNING: Self = Self(900);
    pub const INVALID_PARAMETER: Self = Self(901);
    pub const SESSION_CORRUPT: Self = Self(902);

    pub fn code(&self) -> u16 {
        self.0
    }

    pub fn description(&self) -> &'static str {
        match self.0 {
            100 => "Unknown error",
            101 => "Download cancelled",
            102 => "Checksum verification failed",

            200 => "DNS resolution failed",
            201 => "Connection refused by remote host",
            202 => "Connection reset by remote host",
            203 => "Connection timed out",
            204 => "Network unreachable",
            205 => "TLS/SSL handshake failed",
            206 => "Proxy connection failed",

            300 => "HTTP 401 Unauthorized",
            301 => "HTTP 403 Forbidden",
            302 => "HTTP 404 Not Found",
            303 => "HTTP 416 Range Not Satisfiable",
            304 => "HTTP 429 Too Many Requests",
            305 => "HTTP 5xx Server Error",
            306 => "HTTP 503 Service Unavailable",
            307 => "Too many redirects",
            308 => "HTTP response error",
            315 => "Cloudflare challenge detected",

            400 => "Disk full or quota exceeded",
            401 => "Permission denied",
            402 => "File not found on disk",
            403 => "File already exists",
            404 => "Invalid file path",
            405 => "I/O error",

            500 => "Failed to resolve torrent metadata",
            501 => "No seeds available for torrent",
            502 => "Invalid torrent file",
            503 => "BitTorrent v2 piece layers unavailable",
            510 => "ED2K server unreachable",
            511 => "File not found on ED2K network",
            520 => "Failed to parse M3U8 playlist",
            521 => "Failed to download M3U8 segment",
            522 => "Failed to decrypt M3U8 segment",
            530 => "FTP login failed",
            531 => "File not found on FTP server",
            532 => "FTP transfer failed",
            533 => "SFTP authentication failed",
            534 => "SFTP host key verification failed",
            540 => "yt-dlp not found in PATH",
            541 => "Media authentication required",
            542 => "Media format unavailable",

            600 => "Out of memory",
            601 => "Too many concurrent connections",

            900 => "Engine not running",
            901 => "Invalid parameter",
            902 => "Session data corrupt",

            _ => "Unknown error",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Classify an error message string into an appropriate error code.
///
/// For HTTP downloads, also pass the protocol kind for better classification.
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
    }
    if lower.contains("401") || lower.contains("unauthorized") {
        return ErrorCode::HTTP_UNAUTHORIZED;
    }
    if lower.contains("403") || lower.contains("forbidden") {
        return ErrorCode::HTTP_FORBIDDEN;
    }
    if lower.contains("404") || lower.contains("not found") {
        // Distinguish HTTP 404 from file-not-found on disk
        if protocol == "http" || protocol == "m3u8" {
            return ErrorCode::HTTP_NOT_FOUND;
        }
    }
    if lower.contains("416") || lower.contains("range not satisfiable") {
        return ErrorCode::HTTP_RANGE_NOT_SATISFIABLE;
    }
    if lower.contains("429") || lower.contains("too many requests") {
        return ErrorCode::HTTP_TOO_MANY_REQUESTS;
    }
    if lower.contains("503") || lower.contains("service unavailable") {
        return ErrorCode::HTTP_SERVICE_UNAVAILABLE;
    }
    if lower.contains("5xx")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("server error")
    {
        return ErrorCode::HTTP_SERVER_ERROR;
    }
    if lower.contains("redirect") {
        return ErrorCode::HTTP_REDIRECT_LOOP;
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
        "ftp" | "sftp" => {
            if lower.contains("login") || lower.contains("530") {
                return ErrorCode::FTP_LOGIN_FAILED;
            }
            if lower.contains("auth") {
                return ErrorCode::SFTP_AUTH_FAILED;
            }
            if lower.contains("host key") {
                return ErrorCode::SFTP_HOST_KEY_FAILED;
            }
            if lower.contains("not found") || lower.contains("550") {
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
            if lower.contains("http error 404") || (lower.contains("404") && lower.contains("http")) {
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
    fn classify_media_wins_over_http_403() {
        // A YouTube-specific message that also contains an HTTP status code
        // must be classified by the media arm, not the generic HTTP arm
        assert_eq!(
            classify_error("HTTP Error 403: age-restricted content", "media"),
            ErrorCode::MEDIA_AUTH_REQUIRED
        );
    }
}
