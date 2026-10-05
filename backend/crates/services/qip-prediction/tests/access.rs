//! Who may create, trade or wager, and the property that none of it moves a
//! price.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::{VenueClass, VenueId};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_prediction::access::{AccessBook, EntityRecord, Jurisdiction, VenueKind, VenuePolicy};
use qip_prediction::belief::{OutcomeDistribution, fair_values};
use qip_prediction::market::{EventMarket, FeeSchedule, MarketKind, Outcome, OutcomeId};
use qip_prediction::pricing::Probability;
use qip_prediction::resolution::{
    Comparison, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule, SourceKind,
    UndeterminedRule,
};
use std::collections::{BTreeMap, BTreeSet};

fn t0() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn dec(text: &str) -> Decimal {
    Decimal::parse(text).expect("a decimal literal")
}

fn jur(code: &str) -> Jurisdiction {
    Jurisdiction::new(code).expect("a jurisdiction")
}

fn set<T: Ord>(items: impl IntoIterator<Item = T>) -> BTreeSet<T> {
    items.into_iter().collect()
}

fn policy(venue: &str, kind: VenueKind) -> VenuePolicy {
    VenuePolicy {
        venue: VenueId::new(venue),
        kind,
        creation: set([jur("US-NY")]),
        trading: set([jur("US-NY")]),
        products: set(["binary".to_string()]),
        minimum_age: Some(21),
    }
}

fn entity(venue: &str) -> EntityRecord {
    EntityRecord {
        jurisdiction: jur("us-ny"),
        eligible: true,
        age_years: Some(30),
        venues: set([VenueId::new(venue)]),
    }
}

fn book() -> AccessBook {
    let mut book = AccessBook::new();
    book.set_policy(policy("EVT", VenueKind::Event));
    book.set_policy(policy("WAGER", VenueKind::Wagering));
    book.set_entity("alice", entity("EVT"));
    book
}

fn refusal<T: std::fmt::Debug>(result: qip_core::error::Result<T>) -> String {
    format!("{:?}", result.expect_err("a refusal"))
}

#[test]
fn an_empty_book_refuses_creation_orders_and_wagers_alike() {
    let empty = AccessBook::new();
    let evt = VenueId::new("EVT");
    assert!(empty.admit_creation(&evt, &jur("US-NY")).is_err());
    assert!(empty.admit_event_order("alice", &evt, "binary").is_err());
    assert!(empty.admit_wagering_order("alice", &evt, "binary").is_err());
    // Premise: the same call is admitted once the records exist.
    assert!(book().admit_event_order("alice", &evt, "binary").is_ok());
}

#[test]
fn creation_is_refused_from_a_jurisdiction_the_venue_does_not_permit_and_admitted_from_one_it_does()
{
    let evt = VenueId::new("EVT");
    let permit = book()
        .admit_creation(&evt, &jur(" us-ny "))
        .expect("permitted");
    assert_eq!(permit.jurisdiction().as_str(), "US-NY");
    let refused = refusal(book().admit_creation(&evt, &jur("GB")));
    assert!(refused.contains("GB"), "names the jurisdiction: {refused}");
}

#[test]
fn a_wagering_venue_refuses_the_event_path_and_an_event_venue_refuses_the_wagering_path() {
    let mut b = book();
    b.set_entity("alice", {
        let mut e = entity("WAGER");
        e.venues.insert(VenueId::new("EVT"));
        e
    });
    let wager = VenueId::new("WAGER");
    let evt = VenueId::new("EVT");
    assert!(b.admit_wagering_order("alice", &wager, "binary").is_ok());
    assert!(b.admit_event_order("alice", &evt, "binary").is_ok());
    let crossed = refusal(b.admit_event_order("alice", &wager, "binary"));
    assert!(crossed.contains("Wagering"), "{crossed}");
    assert!(b.admit_wagering_order("alice", &evt, "binary").is_err());
}

#[test]
fn wagering_is_refused_outside_a_permitting_jurisdiction_naming_it() {
    let mut b = book();
    let mut e = entity("WAGER");
    b.set_entity("alice", e.clone());
    assert!(
        b.admit_wagering_order("alice", &VenueId::new("WAGER"), "binary")
            .is_ok()
    );
    e.jurisdiction = jur("FR");
    b.set_entity("alice", e);
    let refused = refusal(b.admit_wagering_order("alice", &VenueId::new("WAGER"), "binary"));
    assert!(refused.contains("FR"), "{refused}");
}

#[test]
fn an_ineligible_unrecorded_or_underage_entity_is_refused_and_a_recorded_adult_is_admitted() {
    let wager = VenueId::new("WAGER");
    let mut b = book();
    assert!(b.admit_wagering_order("ghost", &wager, "binary").is_err());
    let mut e = entity("WAGER");
    b.set_entity("alice", e.clone());
    assert!(b.admit_wagering_order("alice", &wager, "binary").is_ok());
    e.age_years = Some(20);
    b.set_entity("alice", e.clone());
    assert!(refusal(b.admit_wagering_order("alice", &wager, "binary")).contains("is 20"));
    e.age_years = None;
    b.set_entity("alice", e.clone());
    assert!(refusal(b.admit_wagering_order("alice", &wager, "binary")).contains("no recorded age"));
    e.age_years = Some(21);
    e.eligible = false;
    b.set_entity("alice", e);
    assert!(refusal(b.admit_wagering_order("alice", &wager, "binary")).contains("not eligible"));
}

#[test]
fn a_product_the_venue_policy_does_not_list_is_refused() {
    let mut b = book();
    b.set_entity("alice", entity("WAGER"));
    let wager = VenueId::new("WAGER");
    assert!(b.admit_wagering_order("alice", &wager, "binary").is_ok());
    let refused = refusal(b.admit_wagering_order("alice", &wager, "parlay"));
    assert!(refused.contains("parlay"), "{refused}");
}

#[test]
fn a_venue_not_granted_to_the_entity_is_refused_and_granting_it_admits_only_there() {
    let mut b = book();
    let evt = VenueId::new("EVT");
    b.set_policy(policy("EVT2", VenueKind::Event));
    let evt2 = VenueId::new("EVT2");
    // alice holds EVT only.
    assert!(b.admit_event_order("alice", &evt, "binary").is_ok());
    let refused = refusal(b.admit_event_order("alice", &evt2, "binary"));
    assert!(refused.contains("not available"), "{refused}");
    let mut e = entity("EVT");
    e.venues = set([evt2.clone()]);
    b.set_entity("alice", e);
    assert!(b.admit_event_order("alice", &evt2, "binary").is_ok());
    assert!(b.admit_event_order("alice", &evt, "binary").is_err());
}

fn market(published: &str, yes_metric: &str) -> qip_core::error::Result<EventMarket> {
    let criteria = |metric: &str| ResolutionCriteria::Threshold {
        metric: metric.to_string(),
        comparison: Comparison::AtMost,
        value: Decimal::from_int(-25),
    };
    let source =
        ResolutionSource::new("release", SourceKind::Official, vec![published.to_string()]);
    let proposition = Proposition::new(
        "the decision",
        criteria(published),
        source,
        t0().saturating_add(Duration::from_days(30)),
        SettlementRule::new(Decimal::from_int(10), UndeterminedRule::VoidAndRefund)
            .expect("a positive payoff"),
        Duration::from_hours(24),
    )?;
    let yes = Outcome::new(
        OutcomeId::new("yes"),
        "a cut",
        ObjectId::from_string("M-YES"),
        criteria(yes_metric),
    );
    EventMarket::new(
        ObjectId::from_string("M"),
        VenueId::new("EVT"),
        VenueClass::PredictionMarket,
        proposition,
        MarketKind::binary(yes, OutcomeId::new("no"), ObjectId::from_string("M-NO"))?,
        FeeSchedule::FREE,
    )
}

#[test]
fn an_outcome_the_source_does_not_publish_cannot_form_a_market_and_names_the_metric() {
    let refused = refusal(market("rate_change_bp", "unemployment_rate"));
    assert!(refused.contains("unemployment_rate"), "{refused}");
    assert!(market("rate_change_bp", "rate_change_bp").is_ok());
}

#[test]
fn a_price_is_the_same_whether_every_venue_is_available_to_the_entity_or_none_is() {
    let m = market("rate_change_bp", "rate_change_bp").expect("a market");
    let p = |t: &str| Probability::new(dec(t)).expect("a probability");
    let belief = OutcomeDistribution::new(
        BTreeMap::from([
            (OutcomeId::new("yes"), p("0.3")),
            (OutcomeId::new("no"), p("0.7")),
        ]),
        t0(),
        "stated",
    )
    .expect("a distribution");
    let evt = VenueId::new("EVT");
    let refusing = AccessBook::new();
    let none = fair_values(&m, &belief).expect("priced beside a book that refuses");
    let admitting = book();
    let all = fair_values(&m, &belief).expect("priced beside a book that admits");
    // Premise: the entity really is refused in one world and admitted in the other.
    assert!(refusing.admit_event_order("alice", &evt, "binary").is_err());
    assert!(admitting.admit_event_order("alice", &evt, "binary").is_ok());
    assert_eq!(none, all);
    assert_eq!(none.fair_values[&OutcomeId::new("yes")], dec("3"));
    assert_eq!(none.fair_values[&OutcomeId::new("no")], dec("7"));
}
