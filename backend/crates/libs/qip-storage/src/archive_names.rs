//! Object naming and write discipline for the Cloud Storage archive (DATA-072).
//!
//! Objects are partitioned `source=…/date=…/instrument=…/` so a reader scoping
//! a query to one source and one day lists one prefix, and are written once as
//! immutable segments: a re-written segment under a name already referenced by
//! a manifest would silently change history the archive hash-chains. Segments
//! outside the size band are refused, because a flood of tiny objects costs a
//! request each and one enormous object cannot be read in part.

use crate::blob::BlobStore;
use qip_core::error::{Error, Result};

/// Inclusive byte band a segment must fall within.
#[derive(Debug, Clone, Copy)]
pub struct SizeBand {
    pub min: usize,
    pub max: usize,
}

fn component(label: &str, value: &str) -> Result<()> {
    let ok = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && value != "."
        && value != "..";
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "archive {label} {value:?} must be non-empty ASCII letters, digits, '.', '_' or '-' \
             so it cannot escape its partition"
        )))
    }
}

/// UTC civil date from days since 1970-01-01 (Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `source=S/date=YYYY-MM-DD/instrument=I/segment-<sequence, 12 digits>.seg`.
pub fn object_name(source: &str, instrument: &str, ts_secs: u64, sequence: u64) -> Result<String> {
    component("source", source)?;
    component("instrument", instrument)?;
    let days = i64::try_from(ts_secs / 86_400)
        .map_err(|_| Error::invalid("archive timestamp is out of range"))?;
    let (y, m, d) = civil(days);
    Ok(format!(
        "source={source}/date={y:04}-{m:02}-{d:02}/instrument={instrument}/segment-{sequence:012}.seg"
    ))
}

/// Writes closed segments to a [`BlobStore`], once each.
#[derive(Debug)]
pub struct SegmentWriter<'a> {
    store: &'a dyn BlobStore,
    band: SizeBand,
}

impl<'a> SegmentWriter<'a> {
    pub fn new(store: &'a dyn BlobStore, band: SizeBand) -> Result<Self> {
        if band.min == 0 || band.min > band.max {
            return Err(Error::invalid(
                "segment size band needs 0 < min <= max; state both bounds",
            ));
        }
        Ok(Self { store, band })
    }

    /// Returns the object name written.
    // ponytail: check-then-put is not atomic; the Cloud Storage adapter should
    // send `ifGenerationMatch=0` so a racing writer is refused by the service.
    pub fn write(
        &self,
        source: &str,
        instrument: &str,
        ts_secs: u64,
        sequence: u64,
        bytes: Vec<u8>,
    ) -> Result<String> {
        let name = object_name(source, instrument, ts_secs, sequence)?;
        if bytes.len() < self.band.min || bytes.len() > self.band.max {
            return Err(Error::invalid(format!(
                "segment of {} bytes is outside the {}..={} band; buffer more or split",
                bytes.len(),
                self.band.min,
                self.band.max
            )));
        }
        if self.store.get(&name)?.is_some() {
            return Err(Error::denied(format!(
                "segment {name} already exists and segments are immutable; \
                 write the next sequence number instead"
            )));
        }
        self.store.put(&name, bytes)?;
        Ok(name)
    }
}
