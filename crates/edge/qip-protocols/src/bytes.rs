//! Fixed-width primitive reads over a byte slice.
//!
//! Written in-tree rather than pulled from a codec crate because the whole
//! platform is auditable to its leaves, and because every read here is
//! bounds-checked and returns an error instead of panicking. A decoder that
//! panics on a short buffer turns a truncated packet — an everyday event on a
//! multicast feed — into a dead process.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// Which end of a multi-byte integer is transmitted first.
///
/// ITCH and the SBE framing header are big-endian; SBE message bodies are
/// conventionally little-endian. Carrying the order as data rather than as two
/// copies of every reader keeps the two decoders honest about what they assume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ByteOrder {
    /// Most significant byte first: ITCH, and the SBE framing header.
    Big,
    /// Least significant byte first: SBE message bodies, by convention.
    Little,
}

impl ByteOrder {
    fn u64_from(self, bytes: &[u8]) -> u64 {
        let mut value: u64 = 0;
        match self {
            Self::Big => {
                for &b in bytes {
                    value = (value << 8) | u64::from(b);
                }
            }
            Self::Little => {
                for (i, &b) in bytes.iter().enumerate() {
                    value |= u64::from(b) << (8 * i);
                }
            }
        }
        value
    }
}

/// A bounds-checked reader positioned inside a frame.
#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    order: ByteOrder,
}

impl<'a> Reader<'a> {
    /// A reader over `bytes`, reading multi-byte integers in `order`.
    pub fn new(bytes: &'a [u8], order: ByteOrder) -> Self {
        Self { bytes, order }
    }

    /// Bytes available to this reader.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether there is nothing left to read.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The `width` bytes at `offset`, or an error naming what did not fit.
    ///
    /// The error carries the field's extent rather than just "short read": on a
    /// binary feed the only way to find a layout mistake after the fact is to
    /// know which offset the decoder reached for.
    pub fn slice(&self, offset: usize, width: usize) -> Result<&'a [u8]> {
        let end = offset.checked_add(width).ok_or_else(|| {
            Error::schema(format!(
                "field extent {offset}+{width} overflows an address"
            ))
        })?;
        self.bytes.get(offset..end).ok_or_else(|| {
            Error::schema(format!(
                "field at {offset}..{end} is past the end of a {}-byte frame",
                self.bytes.len()
            ))
        })
    }

    /// An unsigned integer of `width` bytes at `offset`.
    pub fn uint(&self, offset: usize, width: usize) -> Result<u64> {
        if width == 0 || width > 8 {
            return Err(Error::schema(format!("unsupported integer width {width}")));
        }
        Ok(self.order.u64_from(self.slice(offset, width)?))
    }

    /// A two's-complement signed integer of `width` bytes.
    pub fn int(&self, offset: usize, width: usize) -> Result<i64> {
        let raw = self.uint(offset, width)?;
        if width == 8 {
            return Ok(raw as i64);
        }
        let sign_bit = 1u64 << (width * 8 - 1);
        if raw & sign_bit == 0 {
            Ok(raw as i64)
        } else {
            // Sign-extend by subtracting 2^(8*width); done arithmetically so the
            // result is identical on every target rather than depending on how
            // the host widens a negative.
            Ok((raw as i64) - ((1i64 << (width * 8 - 1)) << 1))
        }
    }

    /// The single byte at `offset`.
    pub fn u8_at(&self, offset: usize) -> Result<u8> {
        self.slice(offset, 1)?
            .first()
            .copied()
            .ok_or_else(|| Error::schema("empty single-byte read"))
    }

    /// Fixed-width ASCII, right-padded with spaces, as venues encode symbols.
    pub fn ascii(&self, offset: usize, width: usize) -> Result<String> {
        let raw = self.slice(offset, width)?;
        let text: String = raw
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b == b' ' {
                    char::from(b)
                } else {
                    '?'
                }
            })
            .collect();
        Ok(text.trim().to_string())
    }

    /// A price transmitted as an integer with `exponent` implied decimals.
    pub fn fixed(
        &self,
        offset: usize,
        width: usize,
        signed: bool,
        exponent: u32,
    ) -> Result<Decimal> {
        let mantissa = if signed {
            i128::from(self.int(offset, width)?)
        } else {
            i128::from(self.uint(offset, width)?)
        };
        Decimal::from_scaled(mantissa, exponent).ok_or_else(|| {
            Error::numeric(format!(
                "price mantissa {mantissa} with exponent {exponent} is out of range"
            ))
        })
    }
}

/// A venue timestamp expressed as nanoseconds since the session's midnight.
///
/// ITCH transmits six bytes of nanoseconds-since-midnight and nothing else, so
/// the session date has to be supplied by the caller. It is a parameter rather
/// than something the decoder reads from a clock: a decoder that asked the host
/// what day it was would decode a recorded feed differently on replay, which is
/// the one thing this platform is built not to do.
pub fn since_midnight(session_midnight: Timestamp, nanos_of_day: u64) -> Result<Timestamp> {
    let nanos = i64::try_from(nanos_of_day).map_err(|_| {
        Error::schema(format!(
            "nanoseconds-of-day {nanos_of_day} does not fit a timestamp"
        ))
    })?;
    Ok(Timestamp::from_nanos(
        session_midnight.as_nanos().saturating_add(nanos),
    ))
}
