//! Small cross-cutting helpers shared across engine protocol modules

use std::time::{SystemTime, UNIX_EPOCH};
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// If `dir/name` already exists, return `dir/stem.1.ext`, `dir/stem.2.ext`, etc.
pub(crate) fn dedup_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]), // "file.txt" -> ("file", ".txt")
        _ => (name, ""),                                      // "noext" -> ("noext", "")
    };

    for n in 1u32.. {
        let numbered = if ext.is_empty() {
            format!("{stem}.{n}")
        } else {
            format!("{stem}.{n}{ext}")
        };
        let path = dir.join(&numbered);
        if !path.exists() {
            return path;
        }
    }
    // Unreachable in practice
    candidate
}

pub(crate) fn safe_filename(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '|' | '?' | '*' | '\0')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_end().trim_matches('.');
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    if is_windows_device_name(trimmed) {
        return fallback.to_string();
    }
    trimmed.to_string()
}

fn is_windows_device_name(name: &str) -> bool {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    if upper.len() == 4 {
        let b = upper.as_bytes();
        let port = b[3];
        if (b[0..3] == *b"COM" || b[0..3] == *b"LPT") && (b'1'..=b'9').contains(&port) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_replaces_reserved_and_falls_back() {
        assert_eq!(safe_filename("a/b:c", "x"), "a_b_c");
        assert_eq!(safe_filename("...", "fallback"), "fallback");
        assert_eq!(safe_filename("", "fallback"), "fallback");
        assert_eq!(safe_filename("normal.txt", "x"), "normal.txt");
    }

    #[test]
    fn safe_filename_windows_device_names_and_trailing_spaces() {
        assert_eq!(safe_filename("CON", "fallback"), "fallback");
        assert_eq!(safe_filename("NUL.txt", "fallback"), "fallback");
        assert_eq!(safe_filename("com9", "fallback"), "fallback");
        assert_eq!(safe_filename("foo  ", "fallback"), "foo");
    }
}
