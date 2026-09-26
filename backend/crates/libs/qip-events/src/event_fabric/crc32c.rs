//! CRC32C (Castagnoli), the error-detecting code the batch codec uses to catch
//! a bit a disk, a NIC or a memory bus flipped.
//!
//! This is **not** cryptography and makes no claim against a deliberate
//! adversary: the integrity root a verifier trusts is the SHA-256 batch chain
//! (ADR 0043, untouched by this slice). CRC32C exists only because a fast,
//! cheap check per record and per batch catches ordinary corruption without
//! paying a cryptographic hash's cost on every frame — the same trade-off
//! iSCSI (RFC 3720), ext4 and btrfs make for the same reason.
//!
//! The polynomial is Castagnoli's `0x1EDC6F41`, used here in its bit-reflected
//! form `0x82F63B78` because the table below processes the least significant
//! bit first — swapping it for the reflected *IEEE* polynomial `0xEDB88320`
//! (plain CRC-32, e.g. gzip's) is the one-character mistake this module is
//! built to catch, since the two algorithms differ only in that constant and
//! every other line of code still compiles and runs.

/// Castagnoli's polynomial, bit-reflected for a right-shifting, LSB-first
/// table. This is the exact value RFC 3720 §12.1 specifies for iSCSI's
/// header and data digests, and what makes the vectors in
/// `tests/event_fabric_codec.rs` reproducible from the RFC alone.
const POLY: u32 = 0x82F6_3B78;

/// Build the 256-entry lookup table at compile time, so no lazy
/// initialisation or interior mutability is needed for a pure function.
const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 { POLY ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const TABLE: [u32; 256] = build_table();

/// CRC32C of `data`, using the standard iSCSI/ext4/btrfs parameters: initial
/// value all-ones, reflected input and output, final value XORed with
/// all-ones.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = TABLE[index] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}
