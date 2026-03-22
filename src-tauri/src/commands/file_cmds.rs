use std::path::PathBuf;

#[tauri::command]
pub fn reveal_in_folder(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("Path does not exist".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-R", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .args(["/select,", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(parent) = p.parent() {
            open::that(parent.to_string_lossy().as_ref()).map_err(|e| e.to_string())?;
        } else {
            return Err("Path has no parent directory".to_string());
        }
    }

    Ok(())
}

#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    open::that(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn trash_item(path: String) -> Result<(), String> {
    trash::delete(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_binary_file(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| e.to_string())
}

fn normalize_info_hash(raw: &str) -> String {
    let value = raw.trim().to_ascii_lowercase();
    let stripped = value.strip_prefix("urn:btih:").unwrap_or(&value);
    stripped.chars().filter(|c| c.is_ascii_hexdigit()).collect()
}

fn parse_generated_torrent_hash(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    if !lower.ends_with(".torrent") {
        return None;
    }

    let stem = lower.strip_suffix(".torrent")?;
    let hash = stem.strip_prefix("[metadata]").unwrap_or(stem);
    let valid_len = hash.len() == 40 || hash.len() == 64;
    if !valid_len || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    Some(hash.to_string())
}

#[tauri::command]
pub fn trash_generated_torrent_sidecars(dir: String, info_hash: String) -> Result<u32, String> {
    let normalized = normalize_info_hash(&info_hash);

    let mut deleted = 0u32;
    let mut fallback_candidates: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(file_hash) = parse_generated_torrent_hash(file_name) else {
            continue;
        };

        if !normalized.is_empty() && file_hash == normalized {
            let deleted_ok = trash::delete(&path).is_ok() || std::fs::remove_file(&path).is_ok();
            if deleted_ok {
                deleted += 1;
            }
        } else {
            fallback_candidates.push(path);
        }
    }

    if deleted == 0 && fallback_candidates.len() == 1 {
        let path = &fallback_candidates[0];
        if trash::delete(path).is_ok() || std::fs::remove_file(path).is_ok() {
            deleted = 1;
        }
    }

    Ok(deleted)
}
