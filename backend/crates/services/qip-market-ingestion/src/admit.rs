//! Stamp a sensed record with its admission header at the adapter seam
//! (R12-TICK-2053, ADR 0132 decision 1).
//!
//! A `SensedRecord` states its own instants and nothing about where it came
//! from. [`admit`] pairs one with the hash of the bytes it was built from and
//! the entitlement it is used under, through [`Admission::new`], so a record
//! with no traceable source, an unlicensed one, or one received before it
//! happened cannot be built as an `Admitted<SensedRecord>` at all.

use crate::adapter::SensedRecord;
use qip_contracts::admission::{Admissible, Admission, Admitted};
use qip_contracts::governance::Entitlement;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Duration, Timestamp};

/// Admit `record` under `entitlement`, naming `source_hash` as the digest of
/// the bytes it was built from.
///
/// The header's event instant is the record's own, or the instant it
/// occurred where it states none (`occurred_at`); its receive instant is the
/// record's own capture time, or the event instant for a record that states
/// no capture. That last case is a statement that nothing later is known,
/// not a measurement of arrival, and is the same reading `absorb_records`
/// already makes. `uncertainty` is the clock's doubt about both.
pub fn admit(
    record: SensedRecord,
    source_hash: impl Into<String>,
    entitlement: Entitlement,
    uncertainty: Duration,
) -> Result<Admitted<SensedRecord>> {
    let own = record.own_times();
    let event: Timestamp = own.event_time.unwrap_or_else(|| record.occurred_at());
    let receive = own.receive_time.unwrap_or(event);
    let admission = Admission::new(event, receive, event, uncertainty, source_hash, entitlement)?;
    Admitted::admit(record, admission)
}

/// The digest of a record's own canonical serialisation, for an in-tree
/// adapter that has no vendor bytes to hash. It names the record, not a
/// vendor's payload: a connector that holds the bytes it parsed passes the
/// digest of those instead.
pub fn record_digest(record: &SensedRecord) -> Result<String> {
    let bytes = serde_json::to_vec(record)
        .map_err(|e| Error::invalid(format!("a sensed record did not serialise: {e}")))?;
    Ok(sha256_hex(&bytes))
}
