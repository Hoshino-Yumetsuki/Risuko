//! Tiny `.netrc` parser. Loads default user/password credentials keyed by
//! host so HTTP/FTP requests can pick them up without putting secrets in
//! URLs. Only the four directives we care about are recognised: `machine`,
//! `default`, `login`, `password`. `account` and `macdef` are skipped
//!
//! aria2 supports `--netrc-path` and `--no-netrc`; we mirror both option
//! names. The format is whitespace-separated tokens, optionally split
//! across multiple lines — the canonical reference is `man 5 netrc`

use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Default)]
pub struct NetrcEntry {
    pub login: Option<String>,
    pub password: Option<String>,
}

#[derive(Default)]
pub struct Netrc {
    pub machines: HashMap<String, NetrcEntry>,
    pub default: Option<NetrcEntry>,
}

impl Netrc {
    pub fn lookup(&self, host: &str) -> Option<&NetrcEntry> {
        // Keys are stored lowercased on insert, so a single lookup with the
        // lowercased host is enough; no need for an extra exact-case probe
        self.machines
            .get(&host.to_ascii_lowercase())
            .or(self.default.as_ref())
    }

    pub fn parse(input: &str) -> Self {
        let mut out = Netrc::default();
        // `macdef <name>` introduces a macro body that runs until the next
        // blank line. We can't see blank lines after `split_whitespace`, so
        // strip macro bodies from the input first, then tokenise the rest.
        let cleaned = strip_macdefs(input);
        let tokens: Vec<&str> = cleaned.split_whitespace().collect();
        let mut i = 0;
        let mut current_host: Option<String> = None;
        let mut current_entry = NetrcEntry::default();
        let mut in_default = false;

        let flush = |out: &mut Netrc,
                     host: &mut Option<String>,
                     entry: &mut NetrcEntry,
                     is_default: &mut bool| {
            let taken = std::mem::take(entry);
            if *is_default {
                out.default = Some(taken);
                *is_default = false;
            } else if let Some(h) = host.take() {
                out.machines.insert(h.to_ascii_lowercase(), taken);
            }
        };

        while i < tokens.len() {
            match tokens[i] {
                "machine" => {
                    flush(
                        &mut out,
                        &mut current_host,
                        &mut current_entry,
                        &mut in_default,
                    );
                    if i + 1 < tokens.len() {
                        current_host = Some(tokens[i + 1].to_string());
                        i += 2;
                    } else {
                        break;
                    }
                }
                "default" => {
                    flush(
                        &mut out,
                        &mut current_host,
                        &mut current_entry,
                        &mut in_default,
                    );
                    in_default = true;
                    i += 1;
                }
                "login" => {
                    if i + 1 < tokens.len() {
                        current_entry.login = Some(tokens[i + 1].to_string());
                        i += 2;
                    } else {
                        break;
                    }
                }
                "password" | "passwd" => {
                    if i + 1 < tokens.len() {
                        current_entry.password = Some(tokens[i + 1].to_string());
                        i += 2;
                    } else {
                        break;
                    }
                }
                // Recognised-but-ignored directives: skip token + arg
                "account" => {
                    i += if i + 1 < tokens.len() { 2 } else { 1 };
                }
                // `macdef` bodies were already removed by `strip_macdefs`,
                // so any remaining `macdef <name>` is a stray header line
                // Skip the directive plus its name token
                "macdef" => {
                    i += if i + 1 < tokens.len() { 2 } else { 1 };
                }
                _ => {
                    i += 1;
                }
            }
        }
        flush(
            &mut out,
            &mut current_host,
            &mut current_entry,
            &mut in_default,
        );
        out
    }

    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let s = fs::read_to_string(path)?;
        Ok(Self::parse(&s))
    }
}

/// Per `man 5 netrc`, a `macdef <name>` line is followed by macro body
/// lines until the first empty line (which terminates the macro). The body
/// must not be re-tokenised as netrc directives, otherwise tokens like
/// `login` or `password` inside the macro would be misread as credentials
fn strip_macdefs(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_macro = false;
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if in_macro {
            // Macro ends at the first line that is empty (only whitespace).
            if line.trim().is_empty() {
                in_macro = false;
                out.push_str(line);
            }
            // else: drop the body line entirely.
            continue;
        }
        // Match the literal token `macdef` only — `starts_with` alone would
        // also match e.g. `macdefoo` and incorrectly start macro-skip mode
        // The first whitespace-delimited token must equal `macdef`
        if trimmed
            .split_whitespace()
            .next()
            .is_some_and(|tok| tok == "macdef")
        {
            // Drop the `macdef <name>` line itself; body follows.
            in_macro = true;
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Resolve the default netrc location: `$HOME/.netrc` on Unix,
/// `%USERPROFILE%/_netrc` (and `.netrc` as fallback) on Windows
pub fn default_netrc_path() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        if let Ok(home) = std::env::var("USERPROFILE") {
            let p = std::path::PathBuf::from(&home).join("_netrc");
            if p.exists() {
                return Some(p);
            }
            let p = std::path::PathBuf::from(&home).join(".netrc");
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|p| p.join(".netrc"))
            .filter(|p| p.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_machine_entry() {
        let txt = "machine example.com login alice password s3cret\n";
        let n = Netrc::parse(txt);
        let e = n.lookup("example.com").expect("entry");
        assert_eq!(e.login.as_deref(), Some("alice"));
        assert_eq!(e.password.as_deref(), Some("s3cret"));
    }

    #[test]
    fn parses_multiple_entries_and_default() {
        let txt = "\
            machine a.example.com login a password 1\n\
            machine b.example.com\n\
              login b\n\
              password 2\n\
            default login guest password g\n";
        let n = Netrc::parse(txt);
        assert_eq!(
            n.lookup("a.example.com").unwrap().login.as_deref(),
            Some("a")
        );
        assert_eq!(
            n.lookup("b.example.com").unwrap().password.as_deref(),
            Some("2")
        );
        // Unknown host falls back to default.
        assert_eq!(
            n.lookup("c.example.com").unwrap().login.as_deref(),
            Some("guest")
        );
    }

    #[test]
    fn case_insensitive_host_match() {
        let txt = "machine Example.COM login a password p\n";
        let n = Netrc::parse(txt);
        assert!(n.lookup("example.com").is_some());
    }
}
