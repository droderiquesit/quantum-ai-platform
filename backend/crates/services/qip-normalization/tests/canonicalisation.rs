//! Canonicalisation and data contracts.

use qip_core::testing::approx_eq;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_market::bar::{Bar, Interval};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind};
use qip_market::quote::{Quote, Trade, TradeCondition};
use qip_market_ingestion::adapter::SensedRecord;
use qip_normalization::contract::{DataContract, FieldRule, check_all};
use qip_normalization::normalizer::{Normalizer, ScaleGuard, SymbolMapping, UnitConversion};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn object() -> ObjectId {
    ObjectId::from_string("OBJ00000000000000000KSTL")
}

fn quote(venue: &str, bid: &str, ask: &str) -> SensedRecord {
    SensedRecord::Quote(Quote {
        object_id: object(),
        venue: venue.into(),
        at: now(),
        bid: Decimal::parse(bid).unwrap(),
        ask: Decimal::parse(ask).unwrap(),
        bid_size: Decimal::from_int(500),
        ask_size: Decimal::from_int(400),
        quality: Default::default(),
    })
}

// --- venue canonicalisation -------------------------------------------------

#[test]
fn venue_aliases_collapse_onto_the_canonical_code() {
    let normalizer = Normalizer::with_standard_venues();
    for (alias, canonical) in [
        ("NYSE", "XNYS"),
        ("nyse", "XNYS"),
        (" N ", "XNYS"),
        ("NASDAQ", "XNAS"),
        ("LSE", "XLON"),
        ("TSE", "XTKS"),
    ] {
        assert_eq!(
            normalizer.canonical_venue(alias),
            Some(canonical),
            "alias {alias}"
        );
    }
    assert_eq!(normalizer.canonical_venue("UNKNOWN"), None);
}

#[test]
fn records_are_rewritten_onto_canonical_venues() {
    let normalizer = Normalizer::with_standard_venues();
    let (records, report) = normalizer.normalise("acme", vec![quote("NYSE", "10", "10.1")], now());
    assert_eq!(report.venues_canonicalised, 1);
    match &records[0] {
        SensedRecord::Quote(q) => assert_eq!(q.venue, "XNYS"),
        other => panic!("expected a quote, got {other:?}"),
    }

    // A record already canonical is left alone.
    let (_, report) = normalizer.normalise("acme", vec![quote("XNYS", "10", "10.1")], now());
    assert_eq!(report.venues_canonicalised, 0);
    assert!(!report.had_changes());
}

// --- unit conversion --------------------------------------------------------

#[test]
fn the_pence_factor_is_a_hundredth_and_never_the_identity() {
    // The failure this prevents: the factor used to be
    // `Decimal::parse("0.01").unwrap_or(Decimal::ONE)`, and the fallback arm is
    // the identity. An identity factor does not merely leave the price alone —
    // `apply_conversion` returns false for it, so the record passes through
    // unconverted *and* unreported, and every downstream notional on a London
    // name is a hundred times too large with nothing in the report to say so.
    // The premise is asserted first: the factor is not the identity. Without
    // that, the conversion assertions below hold trivially for a no-op.
    let conversion = UnitConversion::pence_to_pounds("lse-feed", object());
    assert_ne!(
        conversion.price_factor,
        Decimal::ONE,
        "premise: a pence-to-pounds conversion that is the identity converts nothing"
    );
    assert_eq!(
        conversion.price_factor * Decimal::from_int(100),
        Decimal::ONE,
        "one pound is a hundred pence, exactly"
    );
    assert_eq!(
        conversion.quantity_factor,
        Decimal::ONE,
        "the re-denomination is of price alone; a share count is a share count"
    );

    let mut normalizer = Normalizer::new();
    normalizer.add_conversion(conversion);
    let (records, report) =
        normalizer.normalise("lse-feed", vec![quote("XLON", "1234", "1236")], now());
    assert_eq!(
        report.units_converted, 1,
        "the conversion must be counted; an identity factor is reported as no conversion at all"
    );
    match &records[0] {
        SensedRecord::Quote(q) => {
            assert_eq!(q.bid, dec!("12.34"));
            assert_eq!(q.ask, dec!("12.36"));
        }
        other => panic!("expected a quote, got {other:?}"),
    }
}

#[test]
fn pence_quotes_are_converted_to_pounds() {
    // The bug this prevents: a London equity quoted at 1234 pence becoming a
    // 1234-pound notional, inflating every position by a hundred times.
    let mut normalizer = Normalizer::with_standard_venues();
    normalizer.add_conversion(UnitConversion::pence_to_pounds("lse-feed", object()));

    let (records, report) =
        normalizer.normalise("lse-feed", vec![quote("LSE", "1234", "1236")], now());
    assert_eq!(report.units_converted, 1);
    match &records[0] {
        SensedRecord::Quote(q) => {
            assert_eq!(q.bid, dec!("12.34"));
            assert_eq!(q.ask, dec!("12.36"));
            assert_eq!(q.bid_size, Decimal::from_int(500), "sizes are unaffected");
        }
        other => panic!("expected a quote, got {other:?}"),
    }

    // A different provider's records are untouched.
    let (records, report) =
        normalizer.normalise("other-feed", vec![quote("LSE", "1234", "1236")], now());
    assert_eq!(report.units_converted, 0);
    match &records[0] {
        SensedRecord::Quote(q) => assert_eq!(q.bid, dec!("1234")),
        other => panic!("expected a quote, got {other:?}"),
    }
}

#[test]
fn conversion_applies_to_every_price_in_a_book_and_a_bar() {
    let mut normalizer = Normalizer::new();
    normalizer.add_conversion(UnitConversion::pence_to_pounds("feed", object()));

    let bar = SensedRecord::Bar(Box::new(Bar {
        object_id: object(),
        venue: "XLON".into(),
        interval: Interval::Minute,
        open_time: now(),
        open: dec!("1000"),
        high: dec!("1100"),
        low: dec!("950"),
        close: dec!("1050"),
        volume: Decimal::from_int(10_000),
        vwap: Some(dec!("1025")),
        trade_count: 42,
        quality: Default::default(),
    }));

    let (records, report) = normalizer.normalise("feed", vec![bar], now());
    assert_eq!(report.units_converted, 1);
    match &records[0] {
        SensedRecord::Bar(b) => {
            assert_eq!(b.open, dec!("10"));
            assert_eq!(b.high, dec!("11"));
            assert_eq!(b.low, dec!("9.5"));
            assert_eq!(b.close, dec!("10.5"));
            assert_eq!(
                b.vwap,
                Some(dec!("10.25")),
                "the vwap must be converted too"
            );
            assert_eq!(b.volume, Decimal::from_int(10_000));
            assert!(b.is_coherent(), "conversion must preserve bar coherence");
        }
        other => panic!("expected a bar, got {other:?}"),
    }
}

// --- symbol mapping ---------------------------------------------------------

#[test]
fn provider_symbols_resolve_to_one_canonical_object() {
    let mut normalizer = Normalizer::new();
    for provider_symbol in ["AAPL", "AAPL.O", "AAPL US Equity"] {
        normalizer.add_symbol_mapping(SymbolMapping {
            provider: "vendor".into(),
            provider_symbol: provider_symbol.into(),
            object_id: ObjectId::from_string("OBJ00000000000000000AAPL"),
            canonical_symbol: "AAPL".into(),
            canonical_venue: "XNAS".into(),
        });
    }

    let mut report = Default::default();
    for spelling in ["AAPL", "aapl.o", " AAPL US EQUITY "] {
        let mapping = normalizer
            .resolve_symbol("vendor", spelling, &mut report)
            .unwrap_or_else(|| panic!("no mapping for {spelling}"));
        assert_eq!(mapping.canonical_symbol, "AAPL");
    }
    assert_eq!(report.symbols_mapped, 3);
    assert_eq!(report.unmapped, 0);
}

#[test]
fn an_unmapped_symbol_is_recorded_rather_than_guessed() {
    let normalizer = Normalizer::new();
    let mut report = Default::default();
    assert!(
        normalizer
            .resolve_symbol("vendor", "MYSTERY", &mut report)
            .is_none()
    );
    assert_eq!(report.unmapped, 1);
    assert_eq!(report.unmapped_symbols.get("vendor:MYSTERY"), Some(&1));
}

// --- timestamps -------------------------------------------------------------

#[test]
fn a_record_stamped_in_the_future_is_clamped() {
    // A future timestamp is a clock or unit error upstream. Left alone it
    // becomes a point-in-time view containing data that did not exist yet.
    let normalizer = Normalizer::new();
    let future = SensedRecord::Trade(Trade {
        object_id: object(),
        venue: "XNYS".into(),
        at: now().saturating_add(Duration::from_days(30)),
        price: dec!("100"),
        size: Decimal::from_int(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    });
    let (records, report) = normalizer.normalise("feed", vec![future], now());
    assert_eq!(report.timestamps_corrected, 1);
    assert_eq!(records[0].occurred_at(), now());
}

#[test]
fn a_future_stamped_bar_is_actually_clamped_not_merely_counted() {
    // The bug this prevents: `clamp_timestamp`'s match fell through to `_ =>
    // {}` for `Bar`, a no-op, while the caller incremented
    // `timestamps_corrected` unconditionally on every kind whenever
    // `occurred_at() > received_at`. The report said the bar's timestamp was
    // fixed while `records[0].occurred_at()` was still 30 days in the future
    // — a false claim about what ran, exactly the failure Principle 6 names.
    // Premise asserted first: the bar's close time is genuinely in the future
    // relative to `received_at`, so a passing test cannot be trivial.
    let received_at = now();
    let bar = SensedRecord::Bar(Box::new(Bar {
        object_id: object(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: received_at.saturating_add(Duration::from_days(30)),
        open: dec!("100"),
        high: dec!("101"),
        low: dec!("99"),
        close: dec!("100.5"),
        volume: Decimal::from_int(1_000),
        vwap: None,
        trade_count: 10,
        quality: Default::default(),
    }));
    assert!(
        bar.occurred_at() > received_at,
        "premise: the bar must actually be future-stamped"
    );

    let normalizer = Normalizer::new();
    let (records, report) = normalizer.normalise("feed", vec![bar], received_at);
    assert_eq!(
        report.timestamps_corrected, 1,
        "a genuinely future bar must be counted as corrected"
    );
    assert_eq!(
        records[0].occurred_at(),
        received_at,
        "the bar's close time must actually move, not just the counter"
    );
}

#[test]
fn a_corporate_action_announced_ahead_of_its_ex_date_is_left_alone() {
    // A split is routinely announced weeks before its ex-date; that ex-date is
    // legitimately later than `received_at` and is not a clock error. Forcing
    // it to "now" would destroy the one fact the record exists to carry —
    // when the entitlement actually takes effect — so normalisation must
    // neither rewrite it nor report a correction that did not happen.
    let received_at = now();
    let action = SensedRecord::CorporateAction(Box::new(CorporateAction {
        object_id: object(),
        ex_date: received_at.saturating_add(Duration::from_days(14)),
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::Split { ratio: dec!("2") },
        announced_at: received_at,
    }));
    assert!(
        action.occurred_at() > received_at,
        "premise: the ex-date must genuinely be ahead of received_at"
    );

    let normalizer = Normalizer::new();
    let (records, report) = normalizer.normalise("feed", vec![action], received_at);
    assert_eq!(
        report.timestamps_corrected, 0,
        "a legitimately future ex-date must not be reported as a correction"
    );
    assert_eq!(
        records[0].occurred_at(),
        received_at.saturating_add(Duration::from_days(14)),
        "the ex-date itself must be untouched"
    );
}

// --- contracts --------------------------------------------------------------

#[test]
fn field_rules_check_presence_and_bounds() {
    let rule = FieldRule::between("spread_bps", 0.0, 100.0);
    assert!(rule.check(Some(50.0)).is_none());
    assert!(
        rule.check(Some(-1.0))
            .unwrap()
            .contains("below the minimum")
    );
    assert!(
        rule.check(Some(500.0))
            .unwrap()
            .contains("above the maximum")
    );
    assert!(rule.check(Some(f64::NAN)).unwrap().contains("not finite"));
    assert!(rule.check(None).unwrap().contains("missing"));

    let optional = FieldRule {
        required: false,
        ..FieldRule::required("x")
    };
    assert!(optional.check(None).is_none());
}

#[test]
fn the_standard_contracts_pass_a_well_formed_record() {
    let contracts = DataContract::standard();
    let violations = check_all(&contracts, &quote("XNYS", "100", "100.1"), now());
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn ratio_rules_cannot_catch_a_unit_error_which_is_why_the_scale_guard_exists() {
    // A whole feed switching from pounds to pence leaves every ratio rule
    // satisfied: a spread in basis points is scale-invariant, so multiplying
    // both sides by a hundred changes nothing it can see.
    let contracts = DataContract::standard();
    let pounds = quote("XLON", "12.34", "12.40");
    let pence = quote("XLON", "1234", "1240");
    assert!(check_all(&contracts, &pounds, now()).is_empty());
    assert!(
        check_all(&contracts, &pence, now()).is_empty(),
        "ratio rules are blind to a change of scale"
    );

    // Continuity against the last accepted price is what catches it.
    let normalizer = Normalizer::new();
    let (_, first) = normalizer.normalise("feed", vec![pounds], now());
    assert!(
        first.scale_warnings.is_empty(),
        "the first observation has nothing to compare to"
    );

    let (_, second) = normalizer.normalise("feed", vec![pence], now());
    assert_eq!(
        second.scale_warnings.len(),
        1,
        "the hundred-fold jump must be flagged"
    );
    assert!(
        second.scale_warnings[0].contains("unit error"),
        "{:?}",
        second.scale_warnings
    );
}

#[test]
fn the_scale_guard_admits_a_violent_but_real_move() {
    let guard = ScaleGuard::default();
    let id = object();
    assert!(
        guard.check(&id, 100.0).is_none(),
        "nothing to compare to yet"
    );
    // A 40% crash in one observation is real and must pass.
    assert!(guard.check(&id, 60.0).is_none());
    // A halving is real too.
    assert!(guard.check(&id, 30.0).is_none());
    // A hundred-fold jump is not.
    assert!(guard.check(&id, 3_000.0).is_some());
    assert_eq!(guard.tracked(), 1);

    // A corporate action legitimately re-bases the price; resetting clears the
    // comparison so the split is not reported as a data error.
    guard.reset(&id);
    assert_eq!(guard.tracked(), 0);
    assert!(guard.check(&id, 1.0).is_none());
}

#[test]
fn the_scale_guard_rejects_a_non_positive_price() {
    let guard = ScaleGuard::default();
    assert!(guard.check(&object(), 0.0).is_some());
    assert!(guard.check(&object(), f64::NAN).is_some());
}

#[test]
fn a_contract_catches_a_stale_record() {
    let contracts = DataContract::standard();
    let stale = quote("XNYS", "100", "100.1");
    let much_later = now().saturating_add(Duration::from_hours(4));
    let violations = check_all(&contracts, &stale, much_later);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].detail.contains("old"), "{violations:?}");
}

#[test]
fn a_contract_catches_an_implausible_bar_range() {
    let contracts = DataContract::standard();
    // A missed split shows up as a bar whose high is many times its low.
    let split_artifact = SensedRecord::Bar(Box::new(Bar {
        object_id: object(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: now(),
        open: dec!("200"),
        high: dec!("200"),
        low: dec!("50"),
        close: dec!("50"),
        volume: Decimal::from_int(1000),
        vwap: None,
        trade_count: 10,
        quality: Default::default(),
    }));
    let violations = check_all(&contracts, &split_artifact, now());
    assert!(
        violations.iter().any(|v| v.detail.contains("range_ratio")),
        "{violations:?}"
    );
}

#[test]
fn a_contract_only_applies_to_its_own_topic() {
    let quote_contract = DataContract::standard()
        .into_iter()
        .find(|c| c.name == "quote-sanity")
        .unwrap();
    let trade = SensedRecord::Trade(Trade {
        object_id: object(),
        venue: "XNYS".into(),
        at: now(),
        price: dec!("100"),
        size: Decimal::from_int(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    });
    assert!(quote_contract.check(&trade, now()).is_passed());
}

#[test]
fn contract_bounds_admit_a_genuinely_violent_but_real_market() {
    // A contract that fired on a real 20% move would be switched off within a
    // week, so the bounds must leave room for markets that actually happen.
    let contracts = DataContract::standard();
    let violent = SensedRecord::Bar(Box::new(Bar {
        object_id: object(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: now(),
        open: dec!("100"),
        high: dec!("105"),
        low: dec!("78"),
        close: dec!("80"),
        volume: Decimal::from_int(50_000_000),
        vwap: Some(dec!("88")),
        trade_count: 900_000,
        quality: Default::default(),
    }));
    let violations = check_all(&contracts, &violent, now());
    assert!(violations.is_empty(), "a 20% crash is real: {violations:?}");

    let wide_but_real = quote("XNYS", "10.00", "10.30");
    assert!(
        check_all(&contracts, &wide_but_real, now()).is_empty(),
        "300bp can be real"
    );
}

#[test]
fn normalisation_reports_what_it_touched() {
    let mut normalizer = Normalizer::with_standard_venues();
    normalizer.add_conversion(UnitConversion::pence_to_pounds("lse-feed", object()));
    let (_, report) = normalizer.normalise(
        "lse-feed",
        vec![quote("LSE", "1234", "1236"), quote("XLON", "1000", "1002")],
        now(),
    );
    assert_eq!(report.processed, 2);
    assert_eq!(report.venues_canonicalised, 1, "only the aliased one");
    assert_eq!(
        report.units_converted, 2,
        "both are from the converting provider"
    );
    assert!(report.had_changes());
    assert!(approx_eq(f64::from(report.processed as u32), 2.0, 1e-12));
}
