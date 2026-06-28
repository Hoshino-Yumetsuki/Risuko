// Path helpers for finding browser cookie databases across platforms

use std::path::{Path, PathBuf};

pub fn expand(path: &str) -> eyre::Result<PathBuf> {
    let expanded = if cfg!(windows) {
        expand_windows(path)
    } else {
        expand_unix(path)
    };
    Ok(PathBuf::from(expanded))
}

#[cfg(unix)]
fn expand_unix(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    path.replace('~', &home).replace("$HOME", &home)
}

#[cfg(not(unix))]
fn expand_unix(_path: &str) -> String {
    String::new()
}

#[cfg(windows)]
fn expand_windows(path: &str) -> String {
    use regex::Regex;
    let re = Regex::new(r"%([^%]+)%").unwrap();
    let mut result = path.to_string();
    for cap in re.captures_iter(path) {
        if let Ok(val) = std::env::var(&cap[1]) {
            result = result.replace(&cap[0], &val);
        }
    }
    result
}

#[cfg(not(windows))]
fn expand_windows(_path: &str) -> String {
    String::new()
}

pub fn find_matching(pattern: &str) -> eyre::Result<Vec<PathBuf>> {
    let expanded = expand(pattern)?;
    let path_str = expanded
        .to_str()
        .ok_or_else(|| eyre::eyre!("invalid path encoding"))?;

    let mut results = Vec::new();
    for p in glob::glob(path_str)?.flatten() {
        results.push(p);
    }
    Ok(results)
}

#[cfg(target_os = "macos")]
pub fn find_first_existing(patterns: &[&str]) -> Option<PathBuf> {
    for pattern in patterns {
        if let Ok(paths) = find_matching(pattern) {
            for p in paths {
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

pub fn find_local_state(cookie_db: &Path) -> Option<PathBuf> {
    // "Local State" sits next to the profile directory or one level above it
    let parent = cookie_db.parent()?;

    for rel in ["../../Local State", "../Local State", "Local State"] {
        let candidate = parent.join(rel);
        if candidate.exists() {
            return candidate.canonicalize().ok();
        }
    }

    None
}
