//! Hand-rolled RSS title parser. Extracts series/season/episode/quality
//! tags from common torrent and anime release naming conventions

use std::sync::OnceLock;

use regex::Regex;

use super::types::ParsedMeta;

struct ParserRegexes {
    season_episode: Regex,   // S01E02 / s1e2 (also S01E02-E04 ranges)
    season_episode_x: Regex, // 1x02 / 01x02
    episode_only: Regex,     // EP01 / Episode 5 / E01
    anime_loose: Regex,      // " - 12 " / " 03 " between brackets/spaces
    year: Regex,             // 1900-2099 standalone
    resolution: Regex,       // 480p / 720p / 1080p / 1440p / 2160p / 4k
    codec: Regex,            // x264/x265/h264/h265/HEVC/AVC/AV1
    source: Regex,           // BluRay/WEB-DL/WEBRip/HDTV/DVDRip/REMUX/UHD
    hdr: Regex,              // HDR/HDR10/HDR10+/Dolby.Vision/DV
    container: Regex,        // .mkv .mp4 .avi .m4v
    group: Regex,            // -GROUP at end (before extension)
    seeders: Regex,          // [123S/45L] or [S:123]
    bracket_group: Regex,    // [Group]
    language: Regex,         // common language tags
}

fn regexes() -> &'static ParserRegexes {
    static R: OnceLock<ParserRegexes> = OnceLock::new();
    R.get_or_init(|| ParserRegexes {
        season_episode: Regex::new(r"(?i)\bS(\d{1,2})E(\d{1,3})(?:[-_ ]?E?(\d{1,3}))?\b").unwrap(),
        season_episode_x: Regex::new(r"(?i)\b(\d{1,2})x(\d{1,3})\b").unwrap(),
        episode_only: Regex::new(r"(?i)\b(?:EP|Episode|E)[ ._-]?(\d{1,3})\b").unwrap(),
        anime_loose: Regex::new(r"(?:^|[\s\-\[\]])(\d{1,3})(?:v\d)?(?:[\s\-\[\]]|$)").unwrap(),
        year: Regex::new(r"\b(19\d{2}|20\d{2})\b").unwrap(),
        resolution: Regex::new(r"(?i)\b(2160p|1440p|1080p|720p|480p|4k|uhd)\b").unwrap(),
        codec: Regex::new(r"(?i)\b(x265|h\.?265|hevc|x264|h\.?264|avc|av1|vp9)\b").unwrap(),
        source: Regex::new(
            r"(?i)\b(BluRay|Blu-Ray|WEB[-_ ]?DL|WEBRip|HDTV|DVDRip|BDRip|BRRip|REMUX|HDRip|CAM|TS)\b",
        )
        .unwrap(),
        hdr: Regex::new(r"(?i)\b(HDR10\+|HDR10|HDR|DV|Dolby[._ ]?Vision)\b").unwrap(),
        container: Regex::new(r"(?i)\.(mkv|mp4|avi|m4v|mov|webm|flv|ts)\b").unwrap(),
        group: Regex::new(r"-([A-Za-z0-9_]+?)(?:\.[A-Za-z0-9]{2,4})?\s*$").unwrap(),
        seeders: Regex::new(r"(?i)\[(?:S[: =]?)?(\d{1,5})\s*[Ss](?:eeders?)?(?:\s*[/|]\s*\d+\s*[Ll])?\]").unwrap(),
        bracket_group: Regex::new(r"^\[([^\]]+)\]").unwrap(),
        language: Regex::new(
            r"(?i)\b(English|Japanese|Korean|Chinese|Spanish|French|German|Italian|Russian|Portuguese|JPN|ENG|CHS|CHT|SUB|DUB|MULTi)\b",
        )
        .unwrap(),
    })
}

/// Normalize a series title for dedupe: lowercase, strip punctuation,
/// collapse whitespace, drop common release tags from the tail
pub fn normalize_series(s: &str) -> String {
    let lowered = s.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_space = false;
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// Parse a release title into structured metadata
pub fn parse_title(raw: &str) -> ParsedMeta {
    let r = regexes();
    let mut meta = ParsedMeta::default();
    let title = raw.trim();

    // Quality tags (resolution, codec, source, hdr)
    let mut quality_tags: Vec<String> = Vec::new();

    if let Some(c) = r.resolution.captures(title) {
        let v = c.get(1).unwrap().as_str().to_lowercase();
        let canon = match v.as_str() {
            "4k" | "uhd" => "2160p".to_string(),
            other => other.to_string(),
        };
        quality_tags.push(canon);
    }
    if let Some(c) = r.codec.captures(title) {
        let raw_codec = c.get(1).unwrap().as_str().to_lowercase();
        let canon = match raw_codec.replace('.', "").as_str() {
            "h265" | "hevc" | "x265" => "x265".to_string(),
            "h264" | "avc" | "x264" => "x264".to_string(),
            other => other.to_string(),
        };
        meta.codec = Some(canon.clone());
        quality_tags.push(canon);
    }
    if let Some(c) = r.source.captures(title) {
        let raw_src = c.get(1).unwrap().as_str();
        let canon = canonical_source(raw_src);
        meta.source = Some(canon.clone());
        quality_tags.push(canon);
    }
    if let Some(c) = r.hdr.captures(title) {
        let v = c.get(1).unwrap().as_str().to_lowercase();
        let canon = if v.contains("dolby") || v == "dv" {
            "DV".to_string()
        } else if v == "hdr10+" {
            "HDR10+".to_string()
        } else if v == "hdr10" {
            "HDR10".to_string()
        } else {
            "HDR".to_string()
        };
        quality_tags.push(canon);
    }
    if let Some(c) = r.container.captures(title) {
        meta.container = Some(c.get(1).unwrap().as_str().to_lowercase());
    }
    if let Some(c) = r.language.captures(title) {
        meta.language = Some(c.get(1).unwrap().as_str().to_string());
    }
    if let Some(c) = r.seeders.captures(title) {
        if let Ok(n) = c.get(1).unwrap().as_str().parse::<u32>() {
            meta.seeders = Some(n);
        }
    }

    meta.quality_tags = quality_tags;

    // Year (avoid swallowing into series). Take the first occurrence
    if let Some(c) = r.year.captures(title) {
        if let Ok(y) = c.get(1).unwrap().as_str().parse::<u32>() {
            meta.year = Some(y);
        }
    }

    // Group: prefer leading [Group] for anime, fall back to -GROUP suffix
    if let Some(c) = r.bracket_group.captures(title) {
        let candidate = c.get(1).unwrap().as_str().trim();
        // Skip pure-numeric brackets (e.g. [1080p])
        if !candidate.chars().all(|ch| ch.is_ascii_digit()) {
            meta.group = Some(candidate.to_string());
        }
    }
    if meta.group.is_none() {
        if let Some(c) = r.group.captures(title) {
            let candidate = c.get(1).unwrap().as_str();
            // Heuristic: avoid matching quality / codec / source-suffix tokens
            // (e.g. the trailing "DL" of "WEB-DL", "RIP" of "BD-RIP")
            let lower = candidate.to_lowercase();
            let is_known_token = matches!(
                lower.as_str(),
                "1080p"
                    | "720p"
                    | "2160p"
                    | "480p"
                    | "1440p"
                    | "x264"
                    | "x265"
                    | "hevc"
                    | "av1"
                    | "dl"
                    | "rip"
                    | "ray"
            );
            // Reject if the token looks like the tail of a hyphenated source
            // tag (WEB-DL, BLU-RAY, BD-RIP, HD-RIP, DVD-RIP, etc.). The group
            // regex matches `-([A-Za-z0-9_]+?)`, so for "WEB-DL" candidate is
            // "DL" preceded by "WEB" \u2014 inspect what's directly before the
            // capture
            let cap0 = c.get(0).unwrap();
            let preceding_is_source_prefix = title[..cap0.start()]
                .rsplit(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .map(|tok| {
                    matches!(
                        tok.to_ascii_uppercase().as_str(),
                        "WEB" | "BLU" | "BD" | "HD" | "DVD" | "BR" | "WEBR"
                    )
                })
                .unwrap_or(false);
            if !is_known_token
                && !preceding_is_source_prefix
                && candidate.chars().any(|c| c.is_alphabetic())
            {
                meta.group = Some(candidate.to_string());
            }
        }
    }

    // Episode/season detection
    let mut ep_match_end: Option<usize> = None;
    if let Some(c) = r.season_episode.captures(title) {
        meta.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        meta.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        ep_match_end = c.get(0).map(|m| m.start());
    } else if let Some(c) = r.season_episode_x.captures(title) {
        meta.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        meta.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        ep_match_end = c.get(0).map(|m| m.start());
    } else if let Some(c) = r.episode_only.captures(title) {
        let n: Option<u32> = c.get(1).and_then(|m| m.as_str().parse().ok());
        meta.absolute_episode = n;
        meta.episode = n;
        ep_match_end = c.get(0).map(|m| m.start());
    } else if let Some(c) = r.anime_loose.captures(title) {
        // Only accept if surrounded by anime-style markers AND title has a
        // bracket group (to reduce false positives)
        if meta.group.is_some() {
            let n: Option<u32> = c.get(1).and_then(|m| m.as_str().parse().ok());
            if let Some(n) = n {
                if (1..=999).contains(&n) {
                    meta.absolute_episode = Some(n);
                    meta.episode = Some(n);
                    ep_match_end = c.get(0).map(|m| m.start());
                }
            }
        }
    }

    // Series name: text before the episode marker (if found) or before the
    // first quality/year token. Strip leading [Group] tag
    let series_end = ep_match_end
        .or_else(|| {
            // Earliest of resolution/source/year start
            [&r.resolution, &r.source, &r.year]
                .iter()
                .filter_map(|re| re.find(title).map(|m| m.start()))
                .min()
        })
        .unwrap_or(title.len());

    let mut series_slice = &title[..series_end];
    // Strip leading bracket group(s)
    while let Some(end) = series_slice.find(']') {
        if series_slice.trim_start().starts_with('[') {
            series_slice = &series_slice[end + 1..];
        } else {
            break;
        }
    }
    // Strip trailing dash/underscore/period clutter
    let cleaned = series_slice
        .trim()
        .trim_end_matches(&['.', '-', '_', ' '][..])
        .trim();
    if !cleaned.is_empty() {
        meta.series = Some(normalize_display_series(cleaned));
    }

    meta
}

fn canonical_source(s: &str) -> String {
    let lower = s.to_lowercase().replace([' ', '_'], "-");
    match lower.as_str() {
        "blu-ray" | "bluray" => "BluRay".into(),
        "web-dl" | "webdl" => "WEB-DL".into(),
        "webrip" => "WEBRip".into(),
        "hdtv" => "HDTV".into(),
        "dvdrip" => "DVDRip".into(),
        "bdrip" => "BDRip".into(),
        "brrip" => "BRRip".into(),
        "remux" => "REMUX".into(),
        "hdrip" => "HDRip".into(),
        other => other.to_uppercase(),
    }
}

/// Display-friendly series: replace underscores/dots with spaces, collapse
/// whitespace. Different from `normalize_series` which is for dedupe
fn normalize_display_series(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        let mapped = if c == '.' || c == '_' { ' ' } else { c };
        if mapped.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(mapped);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_release() {
        let m = parse_title("Show.Name.S01E02.1080p.WEB-DL.x265-GROUP.mkv");
        assert_eq!(m.season, Some(1));
        assert_eq!(m.episode, Some(2));
        assert_eq!(m.codec.as_deref(), Some("x265"));
        assert_eq!(m.source.as_deref(), Some("WEB-DL"));
        assert!(m.quality_tags.iter().any(|t| t == "1080p"));
        assert_eq!(m.container.as_deref(), Some("mkv"));
        assert_eq!(m.group.as_deref(), Some("GROUP"));
        assert_eq!(m.series.as_deref(), Some("Show Name"));
    }

    #[test]
    fn parses_anime_bracket_form() {
        let m = parse_title("[Group] Show 03 (1080p) [HEVC].mkv");
        assert_eq!(m.group.as_deref(), Some("Group"));
        assert_eq!(m.episode, Some(3));
        assert_eq!(m.absolute_episode, Some(3));
        assert!(m.quality_tags.iter().any(|t| t == "1080p"));
        assert_eq!(m.codec.as_deref(), Some("x265")); // hevc → x265
        assert_eq!(m.series.as_deref(), Some("Show"));
    }

    #[test]
    fn parses_movie_with_year_and_hdr() {
        let m = parse_title("Show.Name.2023.2160p.HDR.WEB-DL.AV1-RLSGRP");
        assert_eq!(m.year, Some(2023));
        assert_eq!(m.codec.as_deref(), Some("av1"));
        assert!(m.quality_tags.iter().any(|t| t == "2160p"));
        assert!(m.quality_tags.iter().any(|t| t == "HDR"));
        assert_eq!(m.group.as_deref(), Some("RLSGRP"));
        assert_eq!(m.series.as_deref(), Some("Show Name"));
    }

    #[test]
    fn parses_anime_dash_form() {
        let m = parse_title("[SubsPlease] Anime - 12 [1080p].mkv");
        assert_eq!(m.episode, Some(12));
        assert_eq!(m.absolute_episode, Some(12));
        assert_eq!(m.group.as_deref(), Some("SubsPlease"));
        assert_eq!(m.series.as_deref(), Some("Anime"));
    }

    #[test]
    fn parses_x_form_season() {
        let m = parse_title("Series.Title.1x02.HDTV.x264-AAA");
        assert_eq!(m.season, Some(1));
        assert_eq!(m.episode, Some(2));
        assert_eq!(m.source.as_deref(), Some("HDTV"));
    }

    #[test]
    fn parses_seeders_bracket() {
        let m = parse_title("Some.Release.1080p [123S/45L]");
        assert_eq!(m.seeders, Some(123));
    }

    #[test]
    fn normalize_collapses_punctuation() {
        assert_eq!(normalize_series("Show.Name!  - 2023"), "show name 2023");
    }
}
