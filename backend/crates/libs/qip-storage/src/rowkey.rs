//! Hotspot-safe row keys for a wide-column time-series table (DATA-070).
//!
//! A key that begins with a timestamp sends every write of the present moment
//! to the one tablet that owns "now", so the table's write throughput is that
//! of a single node however many it has. The key therefore begins with the
//! entity, which is high-cardinality and not monotonic, and the time follows
//! it, reversed so that the newest row of an entity sorts first and "latest as
//! of" is a one-row scan.
//!
//! This is the pure naming function only. No Bigtable client exists in-tree
//! (ADR 0009), so nothing here reaches a store; it is the scheme a future
//! adapter is bound to, fixed and tested ahead of it.

use qip_core::error::{Error, Result};

/// Separator between the entity and the time. Entities may not contain it,
/// so a key parses back unambiguously.
const SEP: char = '#';

/// `entity#<u64::MAX - ts_ms, 16 hex digits>`.
pub fn row_key(entity: &str, ts_ms: u64) -> Result<String> {
    if entity.is_empty() || entity.contains(SEP) {
        return Err(Error::invalid(format!(
            "row-key entity {entity:?} must be non-empty and must not contain '{SEP}'; \
             choose an instrument or entity identifier"
        )));
    }
    Ok(format!("{entity}{SEP}{:016x}", u64::MAX - ts_ms))
}

/// Inverse of [`row_key`]; refuses anything it did not produce.
pub fn parse_row_key(key: &str) -> Result<(String, u64)> {
    let (entity, hex) = key
        .rsplit_once(SEP)
        .ok_or_else(|| Error::invalid("row key has no entity/time separator"))?;
    if entity.is_empty() || entity.contains(SEP) || hex.len() != 16 {
        return Err(Error::invalid("row key is not entity#16-hex-digit-time"));
    }
    let reversed = u64::from_str_radix(hex, 16)
        .map_err(|_| Error::invalid("row key time is not hexadecimal"))?;
    Ok((entity.to_string(), u64::MAX - reversed))
}
