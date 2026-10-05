//! CICD-072: a schema version retires only after a consumer inventory proves
//! nothing still depends on it.

use qip_events::{ConsumerInventory, Topic};

#[test]
fn retirement_is_refused_while_any_consumer_is_listed_and_admitted_once_none_is() {
    let mut inv = ConsumerInventory::default();
    inv.depend(Topic::MarketTick, 1, "risk-engine");
    inv.depend(Topic::MarketTick, 1, "backtester");

    let error = inv
        .retire(Topic::MarketTick, 1)
        .expect_err("two consumers listed");
    // Names both consumers, in sorted order, so the operator knows who to chase.
    assert!(
        error.message().contains("[backtester, risk-engine]"),
        "{}",
        error.message()
    );
    assert!(!inv.is_retired(Topic::MarketTick, 1));

    inv.migrate_off(Topic::MarketTick, 1, "risk-engine");
    assert!(
        inv.retire(Topic::MarketTick, 1).is_err(),
        "one consumer remains"
    );
    inv.migrate_off(Topic::MarketTick, 1, "backtester");

    // Premise of the admit path: the inventory exists and is now empty.
    inv.retire(Topic::MarketTick, 1)
        .expect("an empty surveyed inventory proves retirement");
    assert!(inv.is_retired(Topic::MarketTick, 1));
}

#[test]
fn a_version_nobody_surveyed_is_refused_because_silence_is_not_proof() {
    let mut inv = ConsumerInventory::default();
    let error = inv
        .retire(Topic::MarketQuote, 1)
        .expect_err("never surveyed");
    assert!(error.message().contains("no consumer inventory"));

    inv.survey(Topic::MarketQuote, 1);
    inv.retire(Topic::MarketQuote, 1)
        .expect("surveyed and empty");
}

#[test]
fn a_consumer_of_one_version_does_not_block_retiring_another() {
    let mut inv = ConsumerInventory::default();
    inv.depend(Topic::MarketTrade, 2, "ledger");
    inv.survey(Topic::MarketTrade, 1);
    inv.retire(Topic::MarketTrade, 1)
        .expect("v1 has no consumer");
    assert!(inv.retire(Topic::MarketTrade, 2).is_err());
}
