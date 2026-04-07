use url::Url;

/// Parsed representation of an M3U8 playlist
#[derive(Debug, Clone)]
pub enum ParsedPlaylist {
    Master {
        variants: Vec<Variant>,
    },
    Media {
        segments: Vec<Segment>,
        media_sequence: u64,
        end_list: bool,
        total_duration: f64,
    },
}

/// A variant stream in a master playlist
#[derive(Debug, Clone)]
pub struct Variant {
    pub bandwidth: u64,
    pub resolution: Option<String>,
    pub codecs: Option<String>,
    pub url: String,
}

/// A media segment in a media playlist
#[derive(Debug, Clone)]
pub struct Segment {
    pub url: String,
    pub byte_range: Option<ByteRange>,
    pub encryption: Option<EncryptionInfo>,
}

/// Byte range for partial segment requests
#[derive(Debug, Clone)]
pub struct ByteRange {
    pub length: u64,
    pub offset: u64,
}

/// Encryption info for AES-128-CBC decryption
#[derive(Debug, Clone)]
pub struct EncryptionInfo {
    pub method: String,
    pub key_uri: String,
    pub iv: Option<Vec<u8>>,
}

/// Check if a URI points to an M3U8 playlist
pub fn is_m3u8_uri(uri: &str) -> bool {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Try parsing as URL to extract path without query/fragment
    if let Ok(parsed) = Url::parse(trimmed) {
        let path = parsed.path().to_lowercase();
        return path.ends_with(".m3u8") || path.ends_with(".m3u");
    }

    // Fallback: strip query params manually
    let lower = trimmed.to_lowercase();
    let path = lower.split('?').next().unwrap_or(&lower);
    let path = path.split('#').next().unwrap_or(path);
    path.ends_with(".m3u8") || path.ends_with(".m3u")
}

/// Resolve a possibly-relative segment URI against the playlist base URL
pub fn resolve_segment_url(base_url: &str, segment_uri: &str) -> Result<String, String> {
    // Already absolute
    if segment_uri.starts_with("http://") || segment_uri.starts_with("https://") {
        return Ok(segment_uri.to_string());
    }

    let base = Url::parse(base_url).map_err(|e| format!("Invalid base URL: {e}"))?;
    let resolved = base
        .join(segment_uri)
        .map_err(|e| format!("Failed to resolve segment URL: {e}"))?;
    Ok(resolved.to_string())
}

/// Fetch and parse an M3U8 playlist from a URL
pub async fn fetch_and_parse_playlist(
    url: &str,
    client: &reqwest::Client,
) -> Result<ParsedPlaylist, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch playlist: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Playlist fetch failed with status {}",
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read playlist body: {e}"))?;

    parse_playlist_bytes(&bytes, url)
}

/// Parse raw M3U8 bytes into a ParsedPlaylist
fn parse_playlist_bytes(bytes: &[u8], base_url: &str) -> Result<ParsedPlaylist, String> {
    let (_, playlist) = m3u8_rs::parse_playlist(bytes)
        .map_err(|e| format!("Failed to parse M3U8 playlist: {e:?}"))?;

    match playlist {
        m3u8_rs::Playlist::MasterPlaylist(master) => {
            let variants = master
                .variants
                .into_iter()
                .map(|v| {
                    let url = resolve_segment_url(base_url, &v.uri).unwrap_or(v.uri);
                    let resolution = v.resolution.map(|r| format!("{}x{}", r.width, r.height));
                    Variant {
                        bandwidth: v.bandwidth,
                        resolution,
                        codecs: v.codecs,
                        url,
                    }
                })
                .collect();
            Ok(ParsedPlaylist::Master { variants })
        }
        m3u8_rs::Playlist::MediaPlaylist(media) => {
            let media_sequence = media.media_sequence as u64;
            let end_list = media.end_list;
            let mut current_encryption: Option<EncryptionInfo> = None;
            let mut total_duration: f64 = 0.0;
            let mut byte_range_offset: u64 = 0;

            let segments = media
                .segments
                .into_iter()
                .map(|seg| {
                    // Update encryption state if segment has a key tag
                    if let Some(ref key) = seg.key {
                        current_encryption = parse_key_tag(key, base_url);
                    }

                    let url = resolve_segment_url(base_url, &seg.uri).unwrap_or(seg.uri);
                    let duration = seg.duration as f64;
                    total_duration += duration;

                    let byte_range = seg.byte_range.map(|br| {
                        let length = br.length as u64;
                        let offset = br.offset.map(|o| o as u64).unwrap_or(byte_range_offset);
                        byte_range_offset = offset + length;
                        ByteRange { length, offset }
                    });

                    Segment {
                        url,
                        byte_range,
                        encryption: current_encryption.clone(),
                    }
                })
                .collect();

            Ok(ParsedPlaylist::Media {
                segments,
                media_sequence,
                end_list,
                total_duration,
            })
        }
    }
}

/// Parse an EXT-X-KEY tag into EncryptionInfo
fn parse_key_tag(key: &m3u8_rs::Key, base_url: &str) -> Option<EncryptionInfo> {
    let method = match key.method {
        m3u8_rs::KeyMethod::None => return None,
        m3u8_rs::KeyMethod::AES128 => "AES-128".to_string(),
        m3u8_rs::KeyMethod::SampleAES => "SAMPLE-AES".to_string(),
        _ => return None,
    };

    let key_uri = key.uri.as_ref()?;
    let resolved_uri = resolve_segment_url(base_url, key_uri).unwrap_or_else(|_| key_uri.clone());

    let iv = key.iv.as_ref().and_then(|iv_str| parse_hex_iv(iv_str));

    Some(EncryptionInfo {
        method,
        key_uri: resolved_uri,
        iv,
    })
}

/// Parse hex IV string like "0x00000000000000000000000000000001" into bytes
fn parse_hex_iv(iv_str: &str) -> Option<Vec<u8>> {
    let hex = iv_str.strip_prefix("0x").or_else(|| iv_str.strip_prefix("0X")).unwrap_or(iv_str);
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = Vec::with_capacity(16);
    for i in (0..32).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).ok()?;
        bytes.push(byte);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_m3u8_uri() {
        assert!(is_m3u8_uri("https://example.com/video.m3u8"));
        assert!(is_m3u8_uri("https://example.com/video.m3u8?token=abc"));
        assert!(is_m3u8_uri("https://example.com/path/index.M3U8"));
        assert!(is_m3u8_uri("https://example.com/video.m3u"));
        assert!(is_m3u8_uri("  https://example.com/video.m3u8  "));
        assert!(!is_m3u8_uri("https://example.com/video.mp4"));
        assert!(!is_m3u8_uri("https://example.com/m3u8/notaplaylist"));
        assert!(!is_m3u8_uri(""));
    }

    #[test]
    fn test_resolve_segment_url_absolute() {
        let result = resolve_segment_url(
            "https://cdn.example.com/hls/master.m3u8",
            "https://other.com/seg0.ts",
        );
        assert_eq!(result.unwrap(), "https://other.com/seg0.ts");
    }

    #[test]
    fn test_resolve_segment_url_relative() {
        let result = resolve_segment_url(
            "https://cdn.example.com/hls/master.m3u8",
            "seg0.ts",
        );
        assert_eq!(result.unwrap(), "https://cdn.example.com/hls/seg0.ts");
    }

    #[test]
    fn test_resolve_segment_url_absolute_path() {
        let result = resolve_segment_url(
            "https://cdn.example.com/hls/master.m3u8",
            "/videos/seg0.ts",
        );
        assert_eq!(result.unwrap(), "https://cdn.example.com/videos/seg0.ts");
    }

    #[test]
    fn test_parse_hex_iv() {
        let iv = parse_hex_iv("0x00000000000000000000000000000001");
        assert!(iv.is_some());
        let bytes = iv.unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(bytes[15], 1);
        assert_eq!(bytes[0], 0);
    }

    #[test]
    fn test_parse_playlist_master() {
        let data = b"#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=720x480\n\
            low.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2560000,RESOLUTION=1280x720\n\
            mid.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=7680000,RESOLUTION=1920x1080\n\
            high.m3u8\n";

        let result = parse_playlist_bytes(data, "https://example.com/hls/master.m3u8");
        assert!(result.is_ok());
        if let ParsedPlaylist::Master { variants } = result.unwrap() {
            assert_eq!(variants.len(), 3);
            assert_eq!(variants[0].bandwidth, 1280000);
            assert_eq!(variants[0].url, "https://example.com/hls/low.m3u8");
            assert_eq!(variants[2].bandwidth, 7680000);
        } else {
            panic!("Expected master playlist");
        }
    }

    #[test]
    fn test_parse_playlist_media() {
        let data = b"#EXTM3U\n\
            #EXT-X-TARGETDURATION:10\n\
            #EXT-X-MEDIA-SEQUENCE:0\n\
            #EXTINF:9.009,\n\
            seg0.ts\n\
            #EXTINF:9.009,\n\
            seg1.ts\n\
            #EXTINF:3.003,\n\
            seg2.ts\n\
            #EXT-X-ENDLIST\n";

        let result = parse_playlist_bytes(data, "https://example.com/hls/playlist.m3u8");
        assert!(result.is_ok());
        if let ParsedPlaylist::Media {
            segments,
            end_list,
            ..
        } = result.unwrap()
        {
            assert!(end_list);
            assert_eq!(segments.len(), 3);
            assert_eq!(segments[0].url, "https://example.com/hls/seg0.ts");
        } else {
            panic!("Expected media playlist");
        }
    }
}
