//! The polled halt flag: §46.2's second kill-switch path, on this node.
//!
//! The first path is the broadcast — a signed `HaltCommand` on the cell's
//! mesh inbox, applied by [`Cell::apply_halt`]. It fails when the mesh does:
//! a wedged central plane, a partition, a downlink whose circuit is open, and
//! the cell keeps trading its envelope with no way to be told to stop. The
//! blueprint's answer is a second path that shares none of that — "Spanner
//! flag polled and Pub/Sub broadcast. Either halts trading". This module is
//! the polled half as a deployed node can have it today: a file on the
//! execution node's own filesystem, read on every pass of the node's loop
//! and handed to [`Cell::apply_polled_halt`]. In deployment the file is a
//! Secret Manager secret mounted onto the node, or a tmpfs path an operator
//! with a shell on the machine can touch; either way, nothing between the
//! operator's hand and the cell goes through `qip-transport`.
//!
//! What the node decides here, and the cell does not, is what a *failure to
//! read* means. The cell is handed a [`PolledHalt`] and never a path, so it
//! is this module that maps the filesystem's answers onto the four readings
//! — and it maps every failure it cannot name onto the one that halts. A
//! missing file is the flag's off state, because that is the shape an
//! operator uses: create to halt, delete to release. A missing *directory*
//! is not: the mount that carries the flag is gone, the wire's state is
//! unknown, and a wire whose state is unknown reads as engaged. The same for
//! a permission error, a path that turns out to be a directory, or content
//! that is not one of the two words. Fail closed, because the alternative is
//! a kill switch that an unmounted volume releases.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_edge::cell::{Cell, PolledHalt};
use std::path::{Path, PathBuf};

/// The environment variable naming the flag's path.
pub const FLAG_VARIABLE: &str = "QIP_HALT_FLAG_PATH";

/// The flag's location, validated once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HaltFlag {
    path: PathBuf,
}

impl HaltFlag {
    /// Name the flag.
    ///
    /// Refuses an empty or relative path. A relative path resolves against
    /// the working directory, so the same configuration would name two
    /// different files under two supervisors, and one of them would be a
    /// flag nobody could ever engage.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path: PathBuf = path.into();
        if path.as_os_str().is_empty() {
            return Err(Error::invalid(format!(
                "configuration: {FLAG_VARIABLE} is empty; name an absolute path, or leave it \
                 unset to run with the broadcast halt alone"
            )));
        }
        if !path.is_absolute() {
            return Err(Error::invalid(format!(
                "configuration: {FLAG_VARIABLE} is {}, a relative path, and would name a \
                 different file under every working directory; name an absolute one",
                path.display()
            )));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the flag as it stands now.
    ///
    /// One `read` and, when the file is absent, one `metadata` on its
    /// directory: two syscalls, no allocation past the flag's own bytes, and
    /// nothing that leaves the machine. That is what makes it safe to call
    /// on every pass.
    pub fn read(&self) -> PolledHalt {
        match std::fs::read(&self.path) {
            Ok(bytes) => PolledHalt::from_content(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match self.path.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() && !parent.is_dir() => {
                        PolledHalt::Unreadable(format!(
                            "the directory {} that carries the flag is missing; the mount is \
                             gone and the wire's state is unknown",
                            parent.display()
                        ))
                    }
                    _ => PolledHalt::Absent,
                }
            }
            Err(error) => {
                PolledHalt::Unreadable(format!("cannot read {}: {error}", self.path.display()))
            }
        }
    }

    /// Read the flag and apply it to the cell, returning what was read so
    /// the caller can report a change.
    pub fn poll(&self, cell: &mut Cell, now: Timestamp) -> PolledHalt {
        let reading = self.read();
        cell.apply_polled_halt(reading.clone(), now);
        reading
    }
}
