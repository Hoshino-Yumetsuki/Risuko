use serde_json::{json, Map, Value};

pub fn system_defaults() -> Map<String, Value> {
    let downloads_dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join("Downloads")))
        .or_else(|| std::env::current_dir().ok().map(|p| p.join("Downloads")))
        .unwrap_or_else(|| std::env::temp_dir().join("Downloads"))
        .to_string_lossy()
        .to_string();

    let mut m = Map::new();
    m.insert("all-proxy".into(), json!(""));
    m.insert("allow-overwrite".into(), json!(false));
    m.insert("auto-file-renaming".into(), json!(true));
    m.insert("bt-exclude-tracker".into(), json!(""));
    m.insert("bt-enable-lpd".into(), json!(true));
    m.insert("bt-force-encryption".into(), json!(false));
    m.insert("bt-load-saved-metadata".into(), json!(true));
    m.insert("bt-save-metadata".into(), json!(true));
    m.insert("bt-tracker".into(), json!(""));
    m.insert("continue".into(), json!(true));
    m.insert("dht-listen-port".into(), json!(26701));
    m.insert("dir".into(), json!(downloads_dir));
    m.insert("ed2k-server".into(), json!("176.123.5.89:4725,45.82.80.155:5687,85.239.33.123:4232,91.208.162.87:4232,145.239.2.134:4661"));
    m.insert("enable-dht".into(), json!(true));
    m.insert("enable-dht6".into(), json!(true));
    m.insert("enable-peer-exchange".into(), json!(true));
    m.insert("follow-torrent".into(), json!(true));
    m.insert("listen-port".into(), json!(21301));
    m.insert("max-concurrent-downloads".into(), json!(5));
    m.insert("max-download-limit".into(), json!(0));
    m.insert("max-overall-download-limit".into(), json!(0));
    m.insert("max-overall-upload-limit".into(), json!(0));
    m.insert("no-proxy".into(), json!(""));
    m.insert("rpc-listen-port".into(), json!(16800));
    m.insert("rpc-secret".into(), json!(""));
    m.insert("remote-time".into(), json!(false));
    m.insert("seed-ratio".into(), json!(0));
    m.insert("seed-time".into(), json!(0));
    m.insert("split".into(), json!(16));
    m.insert(
        "user-agent".into(),
        json!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"),
    );
    m
}

pub fn user_defaults() -> Map<String, Value> {
    let is_macos = cfg!(target_os = "macos");
    let is_not_macos = !is_macos;

    let mut m = Map::new();
    m.insert("auto-detect-low-speed-tasks".into(), json!(false));
    m.insert("auto-check-update".into(), json!(is_macos));
    m.insert("auto-hide-window".into(), json!(false));
    m.insert("auto-retry".into(), json!(false));
    m.insert("auto-retry-interval".into(), json!(5));
    m.insert("auto-retry-strategy".into(), json!("static"));
    m.insert("auto-sync-tracker".into(), json!(true));
    m.insert("favorite-directories".into(), json!([]));
    m.insert("hide-app-menu".into(), json!(is_not_macos));
    m.insert("history-directories".into(), json!([]));
    m.insert("keep-seeding".into(), json!(false));
    m.insert("keep-window-state".into(), json!(false));
    m.insert("last-check-update-time".into(), json!(0));
    m.insert("last-sync-tracker-time".into(), json!(0));
    m.insert("locale".into(), json!("en-US"));
    m.insert("log-level".into(), json!("warn"));
    m.insert("low-speed-threshold".into(), json!(20));
    m.insert("new-task-show-downloading".into(), json!(true));
    m.insert("no-confirm-before-delete-task".into(), json!(false));
    m.insert("open-at-login".into(), json!(false));
    m.insert(
        "protocols".into(),
        json!({"magnet": true, "thunder": false}),
    );
    m.insert(
        "proxy".into(),
        json!({
            "enable": false,
            "server": "",
            "bypass": "",
            "scope": ["download", "update-app", "update-trackers"]
        }),
    );
    m.insert("rpc-host".into(), json!("127.0.0.1"));
    m.insert("resume-all-when-app-launched".into(), json!(false));
    m.insert("run-mode".into(), json!(1));
    m.insert("show-progress-bar".into(), json!(true));
    m.insert("task-notification".into(), json!(true));
    m.insert("theme".into(), json!("auto"));
    m.insert(
        "tracker-source".into(),
        json!([
            "https://cdn.jsdelivr.net/gh/ngosang/trackerslist/trackers_best_ip.txt",
            "https://cdn.jsdelivr.net/gh/ngosang/trackerslist/trackers_best.txt"
        ]),
    );
    m.insert("tray-theme".into(), json!("auto"));
    m.insert("tray-speedometer".into(), json!(is_macos));
    m.insert("m3u8-output-format".into(), json!("ts"));
    m.insert("update-channel".into(), json!("latest"));
    m.insert("window-state".into(), json!({}));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- system_defaults ---

    #[test]
    fn system_defaults_required_keys() {
        let sys = system_defaults();
        let required = [
            "dir",
            "max-concurrent-downloads",
            "rpc-listen-port",
            "rpc-secret",
            "seed-ratio",
            "seed-time",
            "split",
            "user-agent",
            "enable-dht",
            "listen-port",
        ];
        for key in required {
            assert!(sys.contains_key(key), "missing system key: {key}");
        }
    }

    #[test]
    fn system_defaults_sensible_values() {
        let sys = system_defaults();
        assert_eq!(sys.get("max-concurrent-downloads").unwrap(), 5);
        assert_eq!(sys.get("rpc-listen-port").unwrap(), 16800);
        assert_eq!(sys.get("split").unwrap(), 16);
        assert_eq!(sys.get("rpc-secret").unwrap(), "");
    }

    #[test]
    fn system_defaults_dir_is_downloads() {
        let sys = system_defaults();
        let dir = sys.get("dir").unwrap().as_str().unwrap();
        assert!(
            dir.contains("Downloads") || dir.contains("downloads"),
            "dir should contain 'Downloads', got: {dir}"
        );
    }

    // --- user_defaults ---

    #[test]
    fn user_defaults_required_keys() {
        let user = user_defaults();
        let required = [
            "theme",
            "locale",
            "keep-seeding",
            "auto-check-update",
            "rpc-host",
            "m3u8-output-format",
            "tray-theme",
            "log-level",
        ];
        for key in required {
            assert!(user.contains_key(key), "missing user key: {key}");
        }
    }

    #[test]
    fn user_defaults_sensible_values() {
        let user = user_defaults();
        assert_eq!(user.get("theme").unwrap(), "auto");
        assert_eq!(user.get("locale").unwrap(), "en-US");
        assert_eq!(user.get("keep-seeding").unwrap(), false);
        assert_eq!(user.get("rpc-host").unwrap(), "127.0.0.1");
        assert_eq!(user.get("m3u8-output-format").unwrap(), "ts");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn user_defaults_macos_specific() {
        let user = user_defaults();
        assert_eq!(user.get("auto-check-update").unwrap(), true);
        assert_eq!(user.get("hide-app-menu").unwrap(), false);
        assert_eq!(user.get("tray-speedometer").unwrap(), true);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn user_defaults_non_macos_specific() {
        let user = user_defaults();
        assert_eq!(user.get("auto-check-update").unwrap(), false);
        assert_eq!(user.get("hide-app-menu").unwrap(), true);
        assert_eq!(user.get("tray-speedometer").unwrap(), false);
    }
}
