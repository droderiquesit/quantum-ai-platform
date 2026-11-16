//! `FusedDigest` (R12-TICK-2086): a stale or region-refused digest is not
//! read. Mutation: not run (nothing was compiled).

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::fused_digest::{FusedDigest, FusedDigestEntry};
use qip_core::Timestamp;

fn at(s: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + s)
}

fn digest() -> FusedDigest {
    FusedDigest {
        cell: "newyork-1".into(),
        region: "us-east".into(),
        as_of: at(0),
        valid_until: at(300),
        entries: vec![
            FusedDigestEntry {
                instrument: "AAA".into(),
                score: 0.8,
                expires_at: at(100),
            },
            FusedDigestEntry {
                instrument: "BBB".into(),
                score: -0.6,
                expires_at: at(250),
            },
        ],
        source_digest: "d".into(),
    }
}

#[test]
fn a_fresh_digest_for_the_cells_region_is_read() {
    let d = digest();
    let read = d.read(at(10), "us-east").unwrap();
    assert_eq!(read.len(), 2, "premise: both entries are live");
}

#[test]
fn an_entry_past_its_own_expiry_is_not_returned() {
    let d = digest();
    let read = d.read(at(150), "us-east").unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].instrument, "BBB");
}

#[test]
fn a_digest_for_another_region_is_refused() {
    let d = digest();
    assert!(
        d.read(at(10), "us-east").is_ok(),
        "premise: right region reads"
    );
    let err = d.read(at(10), "eu-west").unwrap_err().to_string();
    assert!(err.contains("not eu-west"), "got {err:?}");
}

#[test]
fn a_stale_digest_is_refused_even_if_an_entry_claims_to_be_live() {
    let mut d = digest();
    d.entries[1].expires_at = at(10_000);
    assert!(
        d.read(at(300), "us-east").is_ok(),
        "premise: valid at its edge"
    );
    let err = d.read(at(301), "us-east").unwrap_err().to_string();
    assert!(err.contains("stale digest"), "got {err:?}");
    assert_eq!(d.remaining(at(301)), qip_core::Duration::ZERO);
}

#[test]
fn an_unaddressed_or_backwards_digest_is_refused() {
    let mut d = digest();
    d.region = " ".into();
    assert!(d.validate().is_err());
    let mut d = digest();
    d.valid_until = d.as_of;
    assert!(d.validate().is_err());
}
