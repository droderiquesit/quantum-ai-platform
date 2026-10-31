//! A historical universe keeps what the present one forgets.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{ObjectId, Timestamp};
use qip_financial::history::{UniverseHistory, VenueOutage};

fn day(n: i64) -> Timestamp {
    Timestamp::from_secs(1_700_000_000 + n * 86_400)
}

fn id(s: &str) -> ObjectId {
    ObjectId::from_string(s)
}

fn history() -> Result<UniverseHistory> {
    let mut h = UniverseHistory::new();
    h.list(id("obj-alive"), "ALIV", day(0))?;
    // Delisted after the date the tests ask about: a survivor-only universe
    // would not contain it.
    h.list(id("obj-failed"), "FAIL", day(0))?;
    h.delist(&id("obj-failed"), day(50))?;
    h.list(id("obj-renamed"), "OLDN", day(0))?;
    h.rename(&id("obj-renamed"), "NEWN", day(30))?;
    h.record_outage(VenueOutage {
        venue: "XNAS".into(),
        from: day(10),
        until: day(11),
        reason: "matching engine halt".into(),
    })?;
    Ok(h)
}

/// A running platform learns an outage in two halves — the venue went out
/// now, and came back later — so the history takes an open outage that
/// overlaps every later window until it is closed. Mutation: make
/// `open_outage` write `from` as `until`; the open outage overlaps nothing and
/// the first assertion fails.
#[test]
fn an_open_outage_overlaps_every_later_window_until_it_is_closed() -> Result<()> {
    let mut h = history()?;
    h.open_outage("XLON", day(20), "withdrawn on feasibility evidence")?;
    assert_eq!(
        h.outages_overlapping("XLON", day(400), day(401)).len(),
        1,
        "an open outage must overlap a window long after it began"
    );
    assert!(
        h.outages_overlapping("XLON", day(19), day(20)).is_empty(),
        "an outage does not reach back before it began"
    );
    assert!(
        h.open_outage("XLON", day(21), "twice").is_err(),
        "one venue is out once at a time"
    );
    assert!(
        h.close_outage("XLON", day(20)).is_err(),
        "a venue cannot come back before it went"
    );
    assert!(
        h.close_outage("XLON", day(25))?,
        "the open outage was closed"
    );
    assert!(!h.close_outage("XLON", day(26))?, "nothing was left open");
    assert!(
        h.outages_overlapping("XLON", day(400), day(401)).is_empty(),
        "a closed outage no longer overlaps later windows"
    );
    assert_eq!(h.outages_overlapping("XLON", day(22), day(23)).len(), 1);
    Ok(())
}

#[test]
fn a_universe_as_of_a_past_date_includes_an_instrument_delisted_afterwards() -> Result<()> {
    let h = history()?;
    let then: Vec<&str> = h
        .members_as_of(day(20))
        .iter()
        .map(|l| l.object_id.as_str())
        .collect();
    assert!(then.contains(&"obj-failed"), "survivorship: {then:?}");
    // The premise the test needs: the same instrument is absent once delisted.
    let later: Vec<&str> = h
        .members_as_of(day(60))
        .iter()
        .map(|l| l.object_id.as_str())
        .collect();
    assert!(!later.contains(&"obj-failed"));
    assert_eq!(later.len(), 2);
    Ok(())
}

#[test]
fn a_symbol_change_resolves_to_one_instrument_on_both_sides() -> Result<()> {
    let h = history()?;
    let before = h.resolve("OLDN", day(5));
    let after = h.resolve("NEWN", day(40));
    assert_eq!(before, Some(&id("obj-renamed")));
    assert_eq!(before, after);
    // Each ticker answers only for its own era.
    assert_eq!(h.resolve("NEWN", day(5)), None);
    assert_eq!(h.resolve("OLDN", day(40)), None);
    Ok(())
}

#[test]
fn a_replay_window_across_a_venue_outage_reports_the_outage() -> Result<()> {
    let h = history()?;
    let across = h.outages_overlapping("XNAS", day(9), day(12));
    assert_eq!(across.len(), 1);
    assert_eq!(across[0].reason, "matching engine halt");
    // The unaffected cases, so the report is not just "always".
    assert!(h.outages_overlapping("XNAS", day(11), day(12)).is_empty());
    assert!(h.outages_overlapping("XNYS", day(9), day(12)).is_empty());
    Ok(())
}

#[test]
fn history_records_that_would_rewrite_it_are_refused() -> Result<()> {
    let mut h = history()?;
    assert!(
        h.delist(&id("obj-failed"), day(60)).is_err(),
        "double delist"
    );
    assert!(
        h.rename(&id("obj-renamed"), "BACK", day(20)).is_err(),
        "rename out of order"
    );
    assert!(
        h.list(id("obj-alive"), "ALIV", day(1)).is_err(),
        "relisting an id"
    );
    assert!(
        h.record_outage(VenueOutage {
            venue: "XNAS".into(),
            from: day(5),
            until: day(5),
            reason: String::new(),
        })
        .is_err()
    );
    Ok(())
}
