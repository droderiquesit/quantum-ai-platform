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
