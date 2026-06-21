// WebKit and Safari timestamp conversion

const WEBKIT_EPOCH_OFFSET: u64 = 11644473600;

pub fn webkit_to_unix(micros: u64) -> Option<u64> {
    if micros == 0 {
        return None;
    }
    let secs = micros / 1_000_000;
    secs.checked_sub(WEBKIT_EPOCH_OFFSET)
}

#[cfg(target_os = "macos")]
pub fn safari_to_unix(timestamp: f64) -> Option<u64> {
    if timestamp <= 0.0 {
        return None;
    }
    let unix = timestamp + 978_307_200.0;
    if unix < 0.0 {
        None
    } else {
        Some(unix as u64)
    }
}
