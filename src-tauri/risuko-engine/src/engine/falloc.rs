//! Cross-platform file pre-allocation
//!
//! On Linux this issues `fallocate(2)` so the kernel reserves real blocks
//! up-front (no later `ENOSPC` mid-download, less fragmentation). On macOS we
//! ask HFS/APFS to pre-allocate via `fcntl(F_PREALLOCATE)` then extend the
//! logical length with `set_len`. On Windows or any failure path we fall back
//! to plain `set_len`, which only updates the file metadata
//!
//! The mode is selectable via the `file-allocation` option:
//!   * `falloc` (default) — try platform fallocate, fall back to `set_len`
//!   * `trunc`            — `set_len` only
//!   * `none`             — neither; writes grow the file as needed
//!
//! `Mode::from_option` accepts the JSON value from the global/task options
//! map and falls back to `Falloc` when the key is missing or invalid

use std::fs::File;
use std::io;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    None,
    Trunc,
    Falloc,
}

impl Mode {
    pub fn from_option(v: Option<&Value>) -> Self {
        match v.and_then(Value::as_str) {
            Some("none") => Mode::None,
            Some("trunc") => Mode::Trunc,
            // Accept both the canonical `falloc` and the aria2-compatible
            // `prealloc` spelling so users migrating configs aren't surprised
            Some("falloc") | Some("prealloc") => Mode::Falloc,
            _ => Mode::Falloc,
        }
    }
}

/// Pre-allocate `file` to `len` bytes according to `mode`. Returns Ok(()) on
/// success. `Falloc` silently degrades to `set_len` on `ENOTSUP`/`EOPNOTSUPP`
/// (network/exotic filesystems) so users on tmpfs/SMB don't see spurious
/// errors
pub fn allocate(file: &File, len: u64, mode: Mode) -> io::Result<()> {
    match mode {
        Mode::None => Ok(()),
        Mode::Trunc => file.set_len(len),
        Mode::Falloc => match platform_fallocate(file, len) {
            Ok(()) => Ok(()),
            Err(e) if is_unsupported(&e) => {
                tracing::debug!(
                    "fallocate not supported on this filesystem ({e}), falling back to set_len"
                );
                file.set_len(len)
            }
            Err(e) => Err(e),
        },
    }
}

#[cfg(target_os = "linux")]
fn platform_fallocate(file: &File, len: u64) -> io::Result<()> {
    use nix::fcntl::{fallocate, FallocateFlags};
    use std::os::unix::io::AsRawFd;
    // 0 flags == reserve blocks AND extend logical length, matching aria2
    fallocate(file.as_raw_fd(), FallocateFlags::empty(), 0, len as i64)
        .map_err(|e| io::Error::from_raw_os_error(e as i32))
}

#[cfg(target_os = "macos")]
fn platform_fallocate(file: &File, len: u64) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // F_PREALLOCATE only reserves blocks; logical size still needs set_len
    // Try contiguous first (F_ALLOCATECONTIG) — if it fails (fragmented free
    // space), retry without the contiguous hint so we still get the reserve
    #[repr(C)]
    struct Fstore {
        fst_flags: libc::c_uint,
        fst_posmode: libc::c_int,
        fst_offset: libc::off_t,
        fst_length: libc::off_t,
        fst_bytesalloc: libc::off_t,
    }
    const F_PREALLOCATE: libc::c_int = 42;
    const F_ALLOCATECONTIG: libc::c_uint = 0x2;
    const F_ALLOCATEALL: libc::c_uint = 0x4;
    const F_PEOFPOSMODE: libc::c_int = 3;

    let fd = file.as_raw_fd();
    let mut store = Fstore {
        fst_flags: F_ALLOCATECONTIG | F_ALLOCATEALL,
        fst_posmode: F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: len as libc::off_t,
        fst_bytesalloc: 0,
    };
    let rc = unsafe { libc::fcntl(fd, F_PREALLOCATE, &mut store as *mut Fstore) };
    if rc == -1 {
        // Retry without contiguous hint.
        store.fst_flags = F_ALLOCATEALL;
        let rc2 = unsafe { libc::fcntl(fd, F_PREALLOCATE, &mut store as *mut Fstore) };
        if rc2 == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    file.set_len(len)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_fallocate(file: &File, len: u64) -> io::Result<()> {
    // No good cross-platform reservation primitive on Windows without
    // Administrator/SeManageVolumePrivilege. Fall back to set_len, which is
    // what `Mode::Trunc` does anyway
    file.set_len(len)
}

#[cfg(unix)]
fn is_unsupported(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc_enotsup) if libc_enotsup == libc::ENOTSUP || libc_enotsup == libc::EOPNOTSUPP
    )
}

#[cfg(not(unix))]
fn is_unsupported(_: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mode_parses() {
        assert_eq!(Mode::from_option(None), Mode::Falloc);
        assert_eq!(Mode::from_option(Some(&json!("none"))), Mode::None);
        assert_eq!(Mode::from_option(Some(&json!("trunc"))), Mode::Trunc);
        assert_eq!(Mode::from_option(Some(&json!("falloc"))), Mode::Falloc);
        assert_eq!(Mode::from_option(Some(&json!("prealloc"))), Mode::Falloc);
        assert_eq!(Mode::from_option(Some(&json!("garbage"))), Mode::Falloc);
    }

    #[test]
    fn allocate_grows_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        let f = std::fs::File::create(&path).unwrap();
        allocate(&f, 4096, Mode::Falloc).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), 4096);
    }

    #[test]
    fn allocate_none_leaves_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        let f = std::fs::File::create(&path).unwrap();
        allocate(&f, 4096, Mode::None).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), 0);
    }
}
