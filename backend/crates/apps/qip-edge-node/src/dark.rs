//! The polled region-availability wire: which peers have gone dark (§36.3).
//!
//! Blueprint §36.3's two region rows say that a region which stops answering
//! goes dark and that **mirrors involving it suspend**, while everything
//! else — local strategies, intra-venue cycles, every other region — carries
//! on. A cell cannot work out on its own that a peer has gone dark: the
//! venues in another region answer this cell whether or not the cell that
//! trades them is alive. Somebody has to say so.
//!
//! # Why it sits beside the halt flag rather than behind a variable of its own
//!
//! The same hand that engages the halt flag is the hand that knows a region
//! has gone dark: an operator draining a region, or the redeploy §36.3's
//! third row calls for. That hand already has one mount on this node —
//! [`crate::halt`]'s flag, named by `QIP_HALT_FLAG_PATH` — and this file
//! lives in the same directory under a fixed name. No second variable, and
//! therefore no second thing a deployment can set inconsistently: a node
//! with a halt wire has a region wire, and a node without one has neither.
//!
//! The absent file is the ordinary state and means nothing is dark, exactly
//! as an absent halt flag means the cell is running. Create to darken,
//! delete to restore.
//!
//! # What a failure to read means, and why it is the opposite of a set
//!
//! The cell is handed a [`RegionOutlook`] and never a path, so it is this
//! module that maps the filesystem's answers onto the readings — and every
//! failure it cannot name maps onto [`RegionOutlook::Unreadable`], which
//! treats **every** region other than this cell's own as dark and suspends
//! every mirror. A missing directory is that: the mount that carries the
//! wire is gone, so is the halt flag beside it, and a wire whose state is
//! unknown is a wire that has failed. A permission error, a path that turns
//! out to be a directory, and content this platform cannot parse are the
//! same.
//!
//! That is fail-closed in the direction that matters here. Mirrored trading
//! is the one thing that depends on a peer being alive; local trading does
//! not, and is untouched. A reading that guessed "nothing is dark" when it
//! could not tell would let this cell take one side of a trade whose other
//! side is nobody, which is the whole failure §36.3's row describes.

use qip_core::Timestamp;
use qip_edge::cell::Cell;
use qip_edge::region::RegionOutlook;
use std::path::{Path, PathBuf};

/// The file's name, inside the directory that carries the halt flag.
pub const DECLARATION_FILE: &str = "dark-regions";

/// The declaration's location, derived once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DarkRegionWire {
    path: PathBuf,
}

impl DarkRegionWire {
    /// The wire beside `halt_flag`, or `None` when the flag names no
    /// directory.
    ///
    /// `None` rather than a guess. A path with no parent cannot be a mounted
    /// flag — `HaltFlag::at` has already refused a relative one — and a
    /// fallback directory chosen here would be a wire an operator could
    /// write to with no effect, which is worse than not having one.
    pub fn beside(halt_flag: &Path) -> Option<Self> {
        let parent = halt_flag.parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        Some(Self {
            path: parent.join(DECLARATION_FILE),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the wire as it stands now.
    ///
    /// One `read` and, when the file is absent, one `metadata` on its
    /// directory: two syscalls, no allocation past the declaration's own
    /// bytes, and nothing that leaves the machine — which is what makes it
    /// safe on every pass, the same argument [`crate::halt`] makes.
    pub fn read(&self) -> RegionOutlook {
        match std::fs::read(&self.path) {
            Ok(bytes) => RegionOutlook::from_content(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match self.path.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() && !parent.is_dir() => {
                        RegionOutlook::Unreadable(format!(
                            "the directory {} that carries the region wire is missing; the mount \
                             is gone and no peer's state is known",
                            parent.display()
                        ))
                    }
                    _ => RegionOutlook::AllLit,
                }
            }
            Err(error) => {
                RegionOutlook::Unreadable(format!("cannot read {}: {error}", self.path.display()))
            }
        }
    }

    /// Read the wire and apply it to the cell, returning what was read so the
    /// caller can report a change.
    pub fn poll(&self, cell: &mut Cell, now: Timestamp) -> RegionOutlook {
        let reading = self.read();
        cell.apply_region_outlook(reading.clone(), now);
        reading
    }
}
