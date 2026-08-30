//! Frame files: creating them, appending to them, and reading them back.
//!
//! A frame file is the file header from [`super::frame`] followed by any
//! number of frames. The write-ahead log and the checkpoint are both frame
//! files; only the meaning of their payloads differs.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::engine::frame::{self, Frame};
use crate::fsio;

/// An open frame file positioned for appends.
#[derive(Debug)]
pub(crate) struct FrameLog {
    file: File,
    /// Bytes currently in the file, header included.
    length: u64,
}

impl FrameLog {
    /// Create a new frame file, replacing any file already at `path`.
    ///
    /// The header is written and flushed before the call returns, so a file
    /// named by a manifest always has a readable header.
    pub(crate) fn create(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        let header = frame::file_header();
        file.write_all(&header)?;
        file.sync_all()?;
        if let Some(parent) = path.parent() {
            fsio::sync_directory(parent);
        }
        Ok(Self {
            file,
            length: header.len() as u64,
        })
    }

    /// Open an existing frame file for appending, positioned at `length`.
    ///
    /// `length` comes from recovery and is the offset just past the last frame
    /// that verified. Anything after it was a torn tail and has been removed.
    pub(crate) fn open_at(path: &Path, length: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(path)
            .map_err(|e| Error::io(format!("cannot open {} for appending: {e}", path.display())))?;
        Ok(Self { file, length })
    }

    /// Bytes of frames, excluding the fixed header.
    pub(crate) fn frame_bytes(&self) -> u64 {
        self.length.saturating_sub(frame::FILE_HEADER_LEN as u64)
    }

    /// Append one payload. Returns the number of bytes written.
    ///
    /// This does **not** flush; the caller decides when to pay for the barrier
    /// so that a batch of frames can share one.
    pub(crate) fn append(&mut self, payload: &[u8]) -> Result<u64> {
        if payload.len() > frame::MAX_PAYLOAD_LEN {
            return Err(Error::invalid(format!(
                "a single record of {} bytes exceeds the {}-byte record limit",
                payload.len(),
                frame::MAX_PAYLOAD_LEN
            )));
        }
        let encoded = frame::encode(payload);
        self.file.write_all(&encoded)?;
        self.length += encoded.len() as u64;
        Ok(encoded.len() as u64)
    }

    /// Flush the file's data and metadata to the storage device.
    pub(crate) fn sync(&self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

    /// Cut the file back to `length`, discarding a torn tail, and flush.
    pub(crate) fn truncate_to(&mut self, length: u64) -> Result<()> {
        self.file.set_len(length)?;
        self.file.sync_all()?;
        self.length = length;
        Ok(())
    }
}

/// The result of scanning a frame file from the header to the end.
#[derive(Clone, Debug)]
pub(crate) struct Scan {
    /// Every complete, digest-verified payload, in file order.
    pub(crate) payloads: Vec<Vec<u8>>,
    /// Offset just past the last verified frame — where appends resume.
    pub(crate) valid_end: u64,
    /// Offset at which an incomplete frame began, if the tail was torn.
    pub(crate) torn_at: Option<u64>,
    /// Bytes after `valid_end`, all of which are discarded.
    pub(crate) discarded: u64,
}

/// Read every frame in the file at `path`.
///
/// A torn tail is reported, not raised: it is the expected shape of a crash.
/// Corruption inside a complete frame is raised, because no truncation can
/// produce it.
pub(crate) fn scan(label: &str, path: &Path) -> Result<Scan> {
    // The path matters more than the errno here: a manifest naming a file that
    // is not there is a very different problem from a permissions mistake, and
    // the operating system's message says neither.
    let bytes = std::fs::read(path).map_err(|e| {
        Error::io(format!(
            "cannot read {}, which the manifest names as live: {e}",
            path.display()
        ))
    })?;
    frame::check_file_header(label, &bytes)?;

    let mut payloads = Vec::new();
    let mut offset = frame::FILE_HEADER_LEN;
    let mut torn_at = None;
    while offset < bytes.len() {
        match frame::read_frame(label, &bytes, offset)? {
            Frame::Complete { payload, end } => {
                payloads.push(payload);
                offset = end;
            }
            Frame::Torn => {
                torn_at = Some(offset as u64);
                break;
            }
        }
    }

    Ok(Scan {
        payloads,
        valid_end: offset as u64,
        torn_at,
        discarded: bytes.len() as u64 - offset as u64,
    })
}

/// The pointer file naming the live checkpoint and log generation.
///
/// It is tiny, rewritten atomically, and self-checked: the digest covers the
/// fields, so a half-written or bit-flipped manifest is refused rather than
/// pointing recovery at the wrong files.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) format_version: u32,
    /// Generation number shared by the live `checkpoint.<n>` and `wal.<n>`.
    pub(crate) generation: u64,
    /// Commit sequence the checkpoint reflects.
    pub(crate) sequence: u64,
    digest: String,
}

impl Manifest {
    pub(crate) fn new(generation: u64, sequence: u64) -> Self {
        let digest = Self::digest_of(frame::FORMAT_VERSION, generation, sequence);
        Self {
            format_version: frame::FORMAT_VERSION,
            generation,
            sequence,
            digest,
        }
    }

    fn digest_of(format_version: u32, generation: u64, sequence: u64) -> String {
        qip_core::hash::sha256_hex(
            format!("qip-storage-manifest|{format_version}|{generation}|{sequence}").as_bytes(),
        )
    }

    pub(crate) fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&text).map_err(|e| {
            Error::schema(format!(
                "the storage manifest at {} is unreadable: {e}",
                path.display()
            ))
        })?;
        if manifest.format_version != frame::FORMAT_VERSION {
            return Err(Error::schema(format!(
                "the storage manifest at {} is format version {}, \
                 this build reads version {}",
                path.display(),
                manifest.format_version,
                frame::FORMAT_VERSION
            )));
        }
        let expected = Self::digest_of(
            manifest.format_version,
            manifest.generation,
            manifest.sequence,
        );
        if manifest.digest != expected {
            return Err(Error::io(format!(
                "the storage manifest at {} failed its own checksum; \
                 it is corrupt and recovery cannot trust which generation is live",
                path.display()
            )));
        }
        Ok(manifest)
    }

    /// Publish the manifest atomically and durably.
    ///
    /// This is the commit point of a checkpoint: before it, recovery uses the
    /// previous generation; after it, the new one. There is no instant at
    /// which it names files that are not fully on disk.
    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_vec_pretty(self)?;
        fsio::write_atomic(path, &text)
    }
}
