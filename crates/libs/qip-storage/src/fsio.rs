//! Durable filesystem primitives.
//!
//! Every file this crate writes goes through here, because the difference
//! between "the kernel accepted the bytes" and "the bytes are on the device" is
//! the difference between an audit trail and a plausible story. A plain
//! `std::fs::write` returns as soon as the data is in the page cache; a power
//! loss a moment later takes it with no error ever having been reported.
//!
//! Three operations cover everything the crate needs:
//!
//! * [`write_atomic`] — replace a file's contents such that a reader either
//!   sees the old bytes or the new ones, never a mixture, and such that the
//!   new bytes survive a crash once the call returns.
//! * [`sync_directory`] — persist the *directory entry* itself. Renaming a
//!   file durably is two steps: fsync the file, then fsync the directory that
//!   names it. Skipping the second can leave a fsynced file with no name.
//! * [`temporary_path`] — a collision-free scratch name beside the target.
//!
//! What this module cannot do is make a lying device tell the truth. `fsync`
//! is a request to the operating system, which forwards it to the drive; a
//! drive with a volatile write cache that ignores flush commands, or a network
//! filesystem that acknowledges early, will defeat it. The guarantee offered
//! here is "we issued every barrier the platform provides", not "physics has
//! been suspended".

use qip_core::error::Result;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes concurrent temporary files targeting the same final path.
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Suffix marking a partially written scratch file.
///
/// Directory listings filter it out: a crash between `create` and `rename`
/// leaves one behind, and it must never be mistaken for stored data.
pub(crate) const TEMPORARY_SUFFIX: &str = ".qip-partial";

/// A scratch path beside `target`, unique within this process.
///
/// The suffix is appended rather than replacing the extension, so
/// `a/b.json` and `a/b.bin` cannot collide with one another.
pub(crate) fn temporary_path(target: &Path) -> PathBuf {
    let sequence = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = target
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(format!("{TEMPORARY_SUFFIX}.{sequence}"));
    target.with_file_name(name)
}

/// Persist a directory's entries.
///
/// Failures are deliberately swallowed. Opening a directory as a file and
/// fsyncing it is the standard Unix idiom, but it is not portable: some
/// platforms refuse the open, others reject the fsync. Treating that as a hard
/// error would make the crate unusable there while adding no durability, and
/// treating it as success would be a lie — so the honest reading of this
/// function is "issue a directory barrier where one is available".
pub(crate) fn sync_directory(directory: &Path) {
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }
}

/// Write `bytes` to `path` atomically and durably.
///
/// On return the new contents are on stable storage under the final name. A
/// crash at any point leaves either the previous contents or the new ones.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        // Before the rename: the data must be durable, or the rename could
        // publish a name pointing at a file whose contents were never flushed.
        file.sync_all()?;
    }
    match std::fs::rename(&temporary, path) {
        Ok(()) => {}
        Err(e) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(e.into());
        }
    }
    if let Some(parent) = path.parent() {
        sync_directory(parent);
    }
    Ok(())
}

/// Whether a path is a leftover scratch file rather than stored data.
pub(crate) fn is_temporary(name: &str) -> bool {
    name.contains(TEMPORARY_SUFFIX)
}
