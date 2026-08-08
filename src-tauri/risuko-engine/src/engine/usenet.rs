//! NZB parsing and non-secret Usenet task metadata

use quick_xml::de::from_str;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsenetProviderProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub security_mode: String,
    pub enabled: bool,
    pub priority: i32,
    pub max_connections: u32,
    pub allow_plain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsenetCredentials {
    pub username: Option<String>,
    pub password: Option<String>,
}

#[async_trait::async_trait]
pub trait UsenetCredentialResolver: Send + Sync {
    async fn resolve(&self, profile_id: &str) -> Result<Option<UsenetCredentials>, String>;
}

pub fn validate_provider_profile(profile: &UsenetProviderProfile) -> Result<(), String> {
    if profile.id.trim().is_empty() || profile.host.trim().is_empty() {
        return Err("Usenet provider id and host are required".into());
    }
    if profile.port == 0 {
        return Err("Usenet provider port must be between 1 and 65535".into());
    }
    if !matches!(
        profile.security_mode.as_str(),
        "implicit-tls" | "starttls" | "plain"
    ) {
        return Err("Unsupported Usenet security mode".into());
    }
    if profile.security_mode == "plain" && !profile.allow_plain {
        return Err("Plain NNTP requires explicit per-profile opt-in".into());
    }
    if profile.max_connections == 0 {
        return Err("Usenet provider max connections must be positive".into());
    }
    Ok(())
}

pub const MAX_NZB_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NZB_FILES: usize = 100_000;
pub const MAX_SEGMENTS_PER_FILE: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NzbSegment {
    pub number: u32,
    pub bytes: u64,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NzbFile {
    pub name: String,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<u64>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub segments: Vec<NzbSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NzbDocument {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub files: Vec<NzbFile>,
}

#[derive(Debug, Deserialize)]
struct RawNzb {
    head: Option<RawHead>,
    #[serde(rename = "file", default)]
    files: Vec<RawFile>,
}

#[derive(Debug, Deserialize)]
struct RawHead {
    #[serde(rename = "meta", default)]
    meta: Vec<RawMeta>,
}

#[derive(Debug, Deserialize)]
struct RawMeta {
    #[serde(rename = "@type")]
    kind: Option<String>,
    #[serde(rename = "$text")]
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(rename = "@poster", default)]
    poster: Option<String>,
    #[serde(rename = "@date", default)]
    date: Option<String>,
    #[serde(rename = "@subject", default)]
    subject: String,
    groups: Option<RawGroups>,
    segments: Option<RawSegments>,
}

#[derive(Debug, Deserialize)]
struct RawGroups {
    #[serde(rename = "group", default)]
    groups: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawSegments {
    #[serde(rename = "segment", default)]
    segments: Vec<RawSegment>,
}

#[derive(Debug, Deserialize)]
struct RawSegment {
    #[serde(rename = "@bytes")]
    bytes: u64,
    #[serde(rename = "@number")]
    number: u32,
    #[serde(rename = "$text")]
    message_id: Option<String>,
}

pub fn parse(bytes: &[u8]) -> Result<NzbDocument, String> {
    if bytes.is_empty() {
        return Err("NZB payload is empty".into());
    }
    if bytes.len() > MAX_NZB_BYTES {
        return Err(format!("NZB payload exceeds {} bytes", MAX_NZB_BYTES));
    }
    let xml = std::str::from_utf8(bytes).map_err(|e| format!("NZB is not UTF-8: {e}"))?;
    let raw: RawNzb = from_str(xml).map_err(|e| format!("NZB parse error: {e}"))?;
    if raw.files.is_empty() {
        return Err("NZB has no files".into());
    }
    if raw.files.len() > MAX_NZB_FILES {
        return Err(format!("NZB contains more than {MAX_NZB_FILES} files"));
    }

    let (title, category) = raw
        .head
        .map(|head| {
            head.meta
                .into_iter()
                .fold((None, None), |(title, category), meta| {
                    let kind = meta.kind.unwrap_or_default();
                    let value = meta.value.unwrap_or_default().trim().to_string();
                    if value.is_empty() {
                        return (title, category);
                    }
                    if (kind.eq_ignore_ascii_case("title") || kind.eq_ignore_ascii_case("name"))
                        && title.is_none()
                    {
                        (Some(value), category)
                    } else if kind.eq_ignore_ascii_case("category") && category.is_none() {
                        (title, Some(value))
                    } else {
                        (title, category)
                    }
                })
        })
        .unwrap_or((None, None));

    let mut files = Vec::with_capacity(raw.files.len());
    for raw_file in raw.files {
        let subject = raw_file.subject.trim().to_string();
        let name = subject_filename(&subject)
            .ok_or_else(|| "NZB file has no usable filename".to_string())?;
        let groups = raw_file
            .groups
            .map(|groups| {
                groups
                    .groups
                    .into_iter()
                    .map(|group| group.trim().to_string())
                    .filter(|group| !group.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if groups.is_empty() {
            return Err(format!("NZB file {name:?} has no newsgroup"));
        }
        let mut segments = raw_file
            .segments
            .ok_or_else(|| format!("NZB file {name:?} has no segments"))?
            .segments
            .into_iter()
            .map(|segment| {
                let message_id = segment
                    .message_id
                    .ok_or_else(|| format!("NZB file {name:?} has a segment without a message id"))?
                    .trim()
                    .to_string();
                if message_id.is_empty() {
                    return Err(format!("NZB file {name:?} has an empty message id"));
                }
                if segment.number == 0 {
                    return Err(format!("NZB file {name:?} has segment number 0"));
                }
                if segment.bytes == 0 {
                    return Err(format!(
                        "NZB file {name:?} has a segment with no byte count"
                    ));
                }
                Ok(NzbSegment {
                    number: segment.number,
                    bytes: segment.bytes,
                    message_id,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if segments.is_empty() {
            return Err(format!("NZB file {name:?} has no usable segments"));
        }
        if segments.len() > MAX_SEGMENTS_PER_FILE {
            return Err(format!("NZB file {name:?} has too many segments"));
        }
        segments.sort_by_key(|segment| segment.number);
        if segments
            .windows(2)
            .any(|window| window[0].number == window[1].number)
        {
            return Err(format!("NZB file {name:?} has duplicate segment numbers"));
        }
        files.push(NzbFile {
            name,
            subject,
            poster: raw_file
                .poster
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            date: raw_file.date.and_then(|value| value.trim().parse().ok()),
            groups,
            segments,
        });
    }

    Ok(NzbDocument {
        title,
        category,
        files,
    })
}

fn subject_filename(subject: &str) -> Option<String> {
    let mut value = subject.trim();
    if let Some((_, suffix)) = value.rsplit_once(" - ") {
        value = suffix.trim();
    }
    for marker in [" yEnc", " yEnc) ", " yEnc]"] {
        if let Some((prefix, _)) = value.split_once(marker) {
            value = prefix.trim();
        }
    }
    value = value.trim_matches(['"', '\'']);
    let value = value
        .replace("[1/1]", "")
        .replace("[1/", "")
        .trim()
        .to_string();
    let base = Path::new(&value)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or(value);
    let safe = crate::engine::util::safe_filename(&base, "");
    (!safe.is_empty()).then_some(safe)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <head><meta type="title">Release Name</meta><meta type="category">TV</meta><meta type="password">private</meta></head>
  <file poster="poster@example" date="1700000000" subject="Release.part01.rar yEnc">
    <groups><group>alt.binaries.example</group></groups>
    <segments><segment bytes="42" number="1">&lt;one@example&gt;</segment></segments>
  </file>
</nzb>"#;

    #[test]
    fn parses_title_groups_and_segments() {
        let doc = parse(SAMPLE).unwrap();
        assert_eq!(doc.title.as_deref(), Some("Release Name"));
        assert_eq!(doc.category.as_deref(), Some("TV"));
        assert_eq!(doc.files[0].name, "Release.part01.rar");
        assert_eq!(doc.files[0].segments[0].message_id, "<one@example>");
        assert_eq!(doc.files[0].date, Some(1_700_000_000));
        let encoded = serde_json::to_string(&doc).unwrap();
        assert!(!encoded.contains("private"));
    }

    #[test]
    fn rejects_missing_group_and_duplicate_numbers() {
        let missing_group = br#"<nzb><file subject="x.bin"><segments><segment number="1">a</segment></segments></file></nzb>"#;
        assert!(parse(missing_group).is_err());
        let duplicate = br#"<nzb><file subject="x.bin"><groups><group>a</group></groups><segments><segment number="1">a</segment><segment number="1">b</segment></segments></file></nzb>"#;
        assert!(parse(duplicate).is_err());

        let missing_id = br#"<nzb><file subject="x.bin"><groups><group>a</group></groups><segments><segment number="1"/></segments></file></nzb>"#;
        assert!(parse(missing_id).is_err());
    }

    #[test]
    fn extracts_quoted_filename_after_a_multipart_subject_prefix() {
        assert_eq!(
            subject_filename(
                r#"reftestnzb 100MB [01/13] - "rar-files.part01.rar" yEnc (1/37) 26214400"#,
            )
            .as_deref(),
            Some("rar-files.part01.rar")
        );
    }

    #[test]
    fn rejects_missing_or_zero_segment_byte_counts() {
        let missing = br#"<nzb><file subject="x.bin"><groups><group>a</group></groups><segments><segment number="1">a</segment></segments></file></nzb>"#;
        assert!(parse(missing).is_err());
        let zero = br#"<nzb><file subject="x.bin"><groups><group>a</group></groups><segments><segment bytes="0" number="1">a</segment></segments></file></nzb>"#;
        assert!(parse(zero).is_err());
    }

    #[test]
    fn rejects_subjects_without_a_usable_filename() {
        let empty = br#"<nzb><file subject="..."><groups><group>a</group></groups><segments><segment bytes="1" number="1">a</segment></segments></file></nzb>"#;
        assert!(parse(empty).is_err());
    }

    #[test]
    fn plain_transport_requires_explicit_opt_in() {
        let mut profile = UsenetProviderProfile {
            id: "p".into(),
            name: "Provider".into(),
            host: "news.example".into(),
            port: 119,
            security_mode: "plain".into(),
            enabled: true,
            priority: 0,
            max_connections: 4,
            allow_plain: false,
            deleted_at: None,
        };
        assert!(validate_provider_profile(&profile).is_err());
        profile.allow_plain = true;
        assert!(validate_provider_profile(&profile).is_ok());
    }
}
