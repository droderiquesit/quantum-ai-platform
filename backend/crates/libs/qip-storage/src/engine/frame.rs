//! The on-disk record format.
//!
//! Both files the engine keeps — the write-ahead log and the checkpoint — are
//! sequences of *frames* behind a fixed file header. One format, one reader,
//! one set of corruption rules.
//!
//! ```text
//! file header (16 bytes)
//!   0   magic          8 bytes   "QIPSTOR\x01"
//!   8   format version 4 bytes   u32 little-endian
//!   12  reserved       4 bytes   zero
//!
//! frame (40 + n bytes)
//!   0   magic          4 bytes   "QWAL"
//!   4   payload length 4 bytes   u32 little-endian
//!   8   digest        32 bytes   SHA-256 of the payload
//!   40  payload        n bytes   JSON-encoded commit
//! ```
//!
//! The digest is a full SHA-256 from [`qip_core::hash`] rather than a shorter
//! checksum. The crate is forbidden third-party dependencies, a CRC would have
//! to be written and validated here from scratch, and the hash the platform
//! already trusts for its audit chain is the one to reuse. Forty bytes of
//! framing per commit is a price worth paying for a corruption check nobody
//! has to review.
//!
//! ## Corruption rules
//!
//! Reading a frame has exactly three outcomes, and the distinction between the
//! last two is the whole point of the format:
//!
//! * **Complete** — magic matched, the declared payload was fully present, and
//!   its digest matched. The payload is returned.
//! * **Torn tail** — the file ends inside the frame, or the remainder of the
//!   file from here is all zero bytes. This is what a crash mid-append or a
//!   power loss during a partial block write looks like. The frame is
//!   discarded and the file truncated back to where it began.
//! * **Corrupt** — the frame is entirely present but its magic or its digest
//!   is wrong. That is not a torn write; it is a device that returned bytes
//!   nobody wrote. It is reported as an error naming the byte offset, and the
//!   store refuses to open. A corrupted record is never returned to a caller
//!   as if it were data.
//!
//! Truncating a file at an arbitrary offset can only ever produce the first
//! two outcomes, which is what makes the truncation tests exhaustive rather
//! than approximate: cutting inside a frame header or payload always leaves
//! fewer bytes than the frame declares.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256;

/// Identifies a file written by this engine.
pub(crate) const FILE_MAGIC: [u8; 8] = *b"QIPSTOR\x01";

/// Bumped only for an incompatible layout change; an older or newer file is
/// rejected rather than guessed at.
pub(crate) const FORMAT_VERSION: u32 = 1;

/// Length of the fixed file header.
pub(crate) const FILE_HEADER_LEN: usize = 16;

/// Marks the start of a frame.
pub(crate) const FRAME_MAGIC: [u8; 4] = *b"QWAL";

/// Length of a frame's header: magic, payload length, digest.
pub(crate) const FRAME_HEADER_LEN: usize = 4 + 4 + 32;

/// Refuse to allocate for an absurd declared length before checking the file.
///
/// A garbage length field would otherwise ask for gigabytes. Anything past the
/// end of the file is a torn tail regardless, so this only bounds the
/// intermediate arithmetic.
pub(crate) const MAX_PAYLOAD_LEN: usize = 256 * 1024 * 1024;

/// The bytes of a file header.
pub(crate) fn file_header() -> Vec<u8> {
    let mut out = Vec::with_capacity(FILE_HEADER_LEN);
    out.extend_from_slice(&FILE_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// Validate a file header, naming what is wrong if it is not ours.
pub(crate) fn check_file_header(label: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() < FILE_HEADER_LEN {
        return Err(Error::io(format!(
            "{label} is {} bytes, shorter than the {FILE_HEADER_LEN}-byte header",
            bytes.len()
        )));
    }
    if bytes[..8] != FILE_MAGIC {
        return Err(Error::io(format!(
            "{label} does not begin with the storage-engine magic; \
             it was not written by this engine"
        )));
    }
    let mut version = [0u8; 4];
    version.copy_from_slice(&bytes[8..12]);
    let version = u32::from_le_bytes(version);
    if version != FORMAT_VERSION {
        return Err(Error::schema(format!(
            "{label} is format version {version}, this build reads version {FORMAT_VERSION}"
        )));
    }
    Ok(())
}

/// Encode one payload as a frame.
pub(crate) fn encode(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    out.extend_from_slice(&FRAME_MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&sha256(payload));
    out.extend_from_slice(payload);
    out
}

/// What reading a frame at some offset produced.
#[derive(Clone, Debug)]
pub(crate) enum Frame {
    /// A complete, digest-verified payload and the offset just past it.
    Complete { payload: Vec<u8>, end: usize },
    /// The frame is incomplete or zero-filled: everything from here is a torn
    /// tail and is discarded.
    Torn,
}

/// Read the frame beginning at `offset`.
///
/// Returns `Err` only for corruption that truncation cannot explain — see the
/// module documentation.
pub(crate) fn read_frame(label: &str, bytes: &[u8], offset: usize) -> Result<Frame> {
    let rest = match bytes.get(offset..) {
        Some(rest) => rest,
        None => return Ok(Frame::Torn),
    };
    if rest.len() < FRAME_HEADER_LEN {
        // Not enough bytes left to even describe a frame.
        return Ok(Frame::Torn);
    }
    if rest[..4] != FRAME_MAGIC {
        // A crash can leave a tail of zeroes where a block was allocated but
        // never written. Anything else at a frame boundary is real damage.
        if rest.iter().all(|b| *b == 0) {
            return Ok(Frame::Torn);
        }
        return Err(Error::io(format!(
            "corrupt record in {label} at byte offset {offset}: \
             expected a frame here, found {:02x?}",
            &rest[..4]
        )));
    }

    let mut length = [0u8; 4];
    length.copy_from_slice(&rest[4..8]);
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_PAYLOAD_LEN {
        return Err(Error::io(format!(
            "corrupt record in {label} at byte offset {offset}: \
             declared payload of {length} bytes exceeds the {MAX_PAYLOAD_LEN}-byte maximum"
        )));
    }
    let end = FRAME_HEADER_LEN + length;
    if rest.len() < end {
        // The file stops inside this frame: a write that never finished.
        return Ok(Frame::Torn);
    }

    let payload = &rest[FRAME_HEADER_LEN..end];
    let expected = &rest[8..40];
    let actual = sha256(payload);
    if expected != actual.as_slice() {
        return Err(Error::io(format!(
            "corrupt record in {label} at byte offset {offset}: \
             digest mismatch over {length} payload bytes \
             (recorded {}, computed {})",
            qip_core::hash::to_hex(&expected[..8]),
            qip_core::hash::to_hex(&actual[..8])
        )));
    }

    Ok(Frame::Complete {
        payload: payload.to_vec(),
        end: offset + end,
    })
}
