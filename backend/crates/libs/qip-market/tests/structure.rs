//! Quotes, books, bars, corporate actions, curves and microstructure.

use qip_core::testing::approx_eq;
use qip_core::{Currency, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_financial::quality::{DECISION_QUALITY_FLOOR, DataQuality};
use qip_market::bar::{Bar, BarSeries, Interval};
use qip_market::book::{BookLevel, OrderBook, Side};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind, adjust_prices};
use qip_market::curve::{CurvePoint, TermStructure};
use qip_market::microstructure::MicrostructureMetrics;
use qip_market::quote::{Quote, Trade, TradeCondition};
use qip_market::snapshot::MarketSnapshot;

fn id(s: &str) -> ObjectId {
    ObjectId::from_string(s)
}

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 22)
}

fn quote(bid: &str, ask: &str, bid_size: i64, ask_size: i64) -> Quote {
    Quote {
        object_id: id("obj-aapl"),
        venue: "XNYS".into(),
        at: now(),
        bid: Decimal::parse(bid).unwrap(),
        ask: Decimal::parse(ask).unwrap(),
        bid_size: Decimal::from_int(bid_size),
        ask_size: Decimal::from_int(ask_size),
        quality: Default::default(),
    }
}

// --- quotes -----------------------------------------------------------------

#[test]
fn mid_and_microprice_differ_when_depth_is_lopsided() {
    let balanced = quote("100.00", "100.10", 500, 500);
    assert_eq!(balanced.mid(), Some(dec!("100.05")));
    assert_eq!(balanced.microprice(), Some(dec!("100.05")));

    // Far more size bid than offered: the microprice leans toward the ask.
    let lopsided = quote("100.00", "100.10", 5000, 100);
    let micro = lopsided.microprice().unwrap();
    assert!(
        micro > lopsided.mid().unwrap(),
        "microprice {micro} should exceed the mid"
    );
    assert!(micro < dec!("100.10"));
    assert!(lopsided.imbalance() > 0.9);
}

#[test]
fn spread_is_reported_in_basis_points() {
    let q = quote("100.00", "100.10", 100, 100);
    assert_eq!(q.spread(), dec!("0.1"));
    // 0.10 on a 100.05 mid is roughly one basis point.
    assert!(approx_eq(q.spread_bps().unwrap(), 9.995, 0.01));
}

#[test]
fn a_crossed_quote_is_flagged_rather_than_treated_as_free_money() {
    let crossed = quote("100.20", "100.10", 100, 100);
    assert!(crossed.is_crossed());
    let issues = crossed.validate();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("crossed"));

    assert!(!quote("100.00", "100.10", 1, 1).is_crossed());
}

#[test]
fn a_one_sided_quote_has_no_mid() {
    let one_sided = quote("100.00", "0", 100, 0);
    assert!(one_sided.mid().is_none());
    assert!(!one_sided.is_two_sided());
    assert!(
        !one_sided.is_crossed(),
        "a missing side is not a crossed market"
    );
}

#[test]
fn trade_aggressor_is_inferred_from_the_prevailing_quote() {
    let q = quote("100.00", "100.10", 100, 100);
    let above = Trade {
        object_id: id("obj-aapl"),
        venue: "XNYS".into(),
        at: now(),
        price: dec!("100.08"),
        size: Decimal::from_int(100),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    };
    assert_eq!(above.infer_aggressor(&q), Some(Side::Buy));

    let below = Trade {
        price: dec!("100.02"),
        ..above.clone()
    };
    assert_eq!(below.infer_aggressor(&q), Some(Side::Sell));

    // At the mid the rule is undefined; guessing would bias signed volume.
    let at_mid = Trade {
        price: dec!("100.05"),
        ..above.clone()
    };
    assert_eq!(at_mid.infer_aggressor(&q), None);

    // A reported side always wins over inference.
    let reported = Trade {
        aggressor: Some(Side::Sell),
        ..above
    };
    assert_eq!(reported.infer_aggressor(&q), Some(Side::Sell));
}

#[test]
fn only_price_forming_conditions_contribute_to_discovery() {
    assert!(TradeCondition::Regular.is_price_forming());
    assert!(TradeCondition::Auction.is_price_forming());
    assert!(!TradeCondition::LateReport.is_price_forming());
    assert!(!TradeCondition::OffExchange.is_price_forming());
    assert!(!TradeCondition::Corrected.is_price_forming());
}

// --- what absence decodes to --------------------------------------------------
//
// Both tests below assert their own premise first. Without it they would pass
// for the wrong reason: "this JSON does not decode" is only interesting once
// you know what it used to decode *to*.

#[test]
fn a_trade_that_states_no_condition_does_not_decode() {
    // The premise. `Regular` is what a `#[serde(default)]` would have
    // supplied — it is the ordinary continuous-session print — and `Regular`
    // is price-forming. So absence used to arrive as permission to move a
    // mark.
    let stated: Trade = serde_json::from_str(TRADE_JSON).expect("a stated condition decodes");
    assert_eq!(stated.condition, TradeCondition::Regular);
    assert!(
        stated.condition.is_price_forming(),
        "the value absence would have produced is the one that forms a price"
    );

    // The same object with the one field removed.
    let without = TRADE_JSON.replace(r#""condition":"regular","#, "");
    assert!(!without.contains("condition"));
    let error = serde_json::from_str::<Trade>(&without)
        .expect_err("a print that says nothing about how it printed is not a regular print");
    assert!(
        error.to_string().contains("condition"),
        "the failure has to name the missing field: {error}"
    );
}

#[test]
fn a_market_record_that_states_no_quality_does_not_decode() {
    // The premise: the default is not "unknown quality", it is a perfect,
    // directly observed measurement that clears the floor deciding whether a
    // record may drive a capital decision.
    let default = DataQuality::default();
    assert!(approx_eq(default.completeness, 1.0, 1e-12));
    assert!(approx_eq(default.confidence, 1.0, 1e-12));
    assert!(!default.is_imputed);
    assert!(
        default.meets(DECISION_QUALITY_FLOOR),
        "an unstated quality block used to clear the decision floor outright"
    );

    // Every market record type carries one, and every one of them refuses.
    let quote_json = r#"{"object_id":"obj-aapl","venue":"XNYS","at":"2026-08-22T00:00:00Z",
        "bid":"10.00","ask":"10.02","bid_size":"100","ask_size":"100"}"#;
    assert!(
        serde_json::from_str::<Quote>(quote_json).is_err(),
        "a quote with no quality block decoded as a perfect measurement"
    );
    assert!(
        serde_json::from_str::<Trade>(&TRADE_JSON.replace(QUALITY_JSON, "null")).is_err(),
        "a trade with a null quality block decoded"
    );

    let without = TRADE_JSON.replace(&format!(r#","quality":{QUALITY_JSON}"#), "");
    assert!(!without.contains("quality"));
    assert!(serde_json::from_str::<Trade>(&without).is_err());

    // And the record that does state it still decodes, so this is a refusal of
    // silence rather than of the field.
    let stated: Trade = serde_json::from_str(TRADE_JSON).expect("a stated quality decodes");
    assert!(stated.quality.meets(DECISION_QUALITY_FLOOR));
}

/// The quality block the fixture states. Extracted so the tests above can
/// remove exactly it and nothing else.
const QUALITY_JSON: &str =
    r#"{"completeness":1.0,"confidence":1.0,"validation_failures":0,"is_imputed":false}"#;

/// A trade as it is actually written, with every field the decoder requires.
const TRADE_JSON: &str = r#"{"object_id":"obj-aapl","venue":"XNYS","at":"2026-08-22T00:00:00Z",
    "price":"10.01","size":"100","condition":"regular","quality":{"completeness":1.0,"confidence":1.0,"validation_failures":0,"is_imputed":false}}"#;

// --- order book -------------------------------------------------------------

fn book() -> OrderBook {
    OrderBook::from_levels(
        id("obj-aapl"),
        "XNYS",
        now(),
        vec![
            BookLevel::new(dec!("100.00"), Decimal::from_int(500)),
            BookLevel::new(dec!("99.99"), Decimal::from_int(800)),
            BookLevel::new(dec!("99.98"), Decimal::from_int(1200)),
        ],
        vec![
            BookLevel::new(dec!("100.02"), Decimal::from_int(400)),
            BookLevel::new(dec!("100.03"), Decimal::from_int(600)),
            BookLevel::new(dec!("100.05"), Decimal::from_int(1000)),
        ],
    )
}

#[test]
fn the_book_orders_each_side_toward_the_touch() {
    let b = book();
    assert_eq!(b.best_bid().unwrap().price, dec!("100.00"));
    assert_eq!(b.best_ask().unwrap().price, dec!("100.02"));
    assert_eq!(b.spread(), Some(dec!("0.02")));
    assert_eq!(b.mid(), Some(dec!("100.01")));
    b.validate().expect("a well-formed book");
}

#[test]
fn depth_and_imbalance_aggregate_across_levels() {
    let b = book();
    assert_eq!(b.depth(Side::Buy, 1), Decimal::from_int(500));
    assert_eq!(b.depth(Side::Buy, 3), Decimal::from_int(2500));
    assert_eq!(b.depth(Side::Sell, 3), Decimal::from_int(2000));
    assert_eq!(b.total_depth(), Decimal::from_int(4500));
    // More bid depth than ask over three levels.
    assert!(b.imbalance(3) > 0.0);
    assert!(b.imbalance(1) > 0.0);
}

#[test]
fn sweeping_the_book_reports_the_price_actually_paid() {
    let b = book();
    // 400 lifts only the touch.
    let (price, filled) = b.sweep(Side::Buy, Decimal::from_int(400)).unwrap();
    assert_eq!(price, dec!("100.02"));
    assert_eq!(filled, Decimal::from_int(400));

    // 1000 walks two levels: 400 @ 100.02 + 600 @ 100.03.
    let (price, filled) = b.sweep(Side::Buy, Decimal::from_int(1000)).unwrap();
    assert_eq!(filled, Decimal::from_int(1000));
    let expected = (dec!("100.02") * Decimal::from_int(400)
        + dec!("100.03") * Decimal::from_int(600))
        / Decimal::from_int(1000);
    assert_eq!(price, expected);
    assert!(price > dec!("100.02"), "sweeping costs more than the touch");
}

#[test]
fn a_sweep_beyond_available_depth_reports_partial_fill() {
    let b = book();
    let (_, filled) = b.sweep(Side::Buy, Decimal::from_int(10_000)).unwrap();
    assert_eq!(filled, Decimal::from_int(2000), "only what is on the book");
    // And the cost is undefined, not extrapolated.
    assert!(
        b.sweep_cost_bps(Side::Buy, Decimal::from_int(10_000))
            .is_none()
    );
}

#[test]
fn sweep_cost_grows_with_size() {
    let b = book();
    let small = b.sweep_cost_bps(Side::Buy, Decimal::from_int(100)).unwrap();
    let large = b
        .sweep_cost_bps(Side::Buy, Decimal::from_int(1500))
        .unwrap();
    assert!(large > small, "small {small} large {large}");
    assert!(small > 0.0, "crossing the spread always costs something");
}

#[test]
fn a_malformed_book_is_rejected() {
    let mut crossed = book();
    crossed.bids[0].price = dec!("100.50");
    assert!(crossed.is_crossed());
    assert!(crossed.validate().is_err());

    let mut unsorted = book();
    unsorted.asks.reverse();
    assert!(unsorted.validate().is_err());

    let mut negative = book();
    negative.bids[0].size = Decimal::from_int(-1);
    assert!(negative.validate().is_err());
}

#[test]
fn the_book_collapses_to_a_top_of_book_quote() {
    let q = book().to_quote().unwrap();
    assert_eq!(q.bid, dec!("100.00"));
    assert_eq!(q.ask_size, Decimal::from_int(400));
    assert_eq!(q.mid(), Some(dec!("100.01")));
}

#[test]
fn an_empty_book_yields_nothing_rather_than_a_default_price() {
    let empty = OrderBook::new(id("obj-x"), "XNYS", now());
    assert!(empty.is_empty());
    assert!(empty.mid().is_none());
    assert!(empty.to_quote().is_none());
    assert!(empty.sweep(Side::Buy, Decimal::ONE).is_none());
}

// --- bars -------------------------------------------------------------------

fn bar(open_offset: i64, open: &str, high: &str, low: &str, close: &str, volume: i64) -> Bar {
    Bar {
        object_id: id("obj-aapl"),
        venue: "XNYS".into(),
        interval: Interval::Minute,
        open_time: now().saturating_add(Duration::from_mins(open_offset)),
        open: Decimal::parse(open).unwrap(),
        high: Decimal::parse(high).unwrap(),
        low: Decimal::parse(low).unwrap(),
        close: Decimal::parse(close).unwrap(),
        volume: Decimal::from_int(volume),
        vwap: None,
        trade_count: 10,
        quality: Default::default(),
    }
}

#[test]
fn bar_coherence_is_checked() {
    assert!(bar(0, "100", "101", "99", "100.5", 1000).is_coherent());
    // High below the close cannot happen.
    assert!(!bar(0, "100", "100.2", "99", "100.5", 1000).is_coherent());
    // Low above the open cannot happen.
    assert!(!bar(0, "100", "101", "100.5", "100.8", 1000).is_coherent());
}

#[test]
fn merging_bars_takes_the_extremes_and_the_later_close() {
    let first = bar(0, "100", "101", "99.5", "100.5", 1000);
    let second = bar(1, "100.5", "102", "100.2", "101.8", 2000);
    let merged = first.merged_with(&second);
    assert_eq!(merged.open, dec!("100"));
    assert_eq!(merged.close, dec!("101.8"));
    assert_eq!(merged.high, dec!("102"));
    assert_eq!(merged.low, dec!("99.5"));
    assert_eq!(merged.volume, Decimal::from_int(3000));
    assert_eq!(merged.trade_count, 20);
}

#[test]
fn a_series_stays_ordered_and_replaces_corrections() {
    let mut series = BarSeries::new();
    series.push(bar(2, "102", "103", "101", "102.5", 100));
    series.push(bar(0, "100", "101", "99", "100.5", 100));
    series.push(bar(1, "101", "102", "100", "101.5", 100));
    assert_eq!(series.len(), 3);
    assert_eq!(series.bars()[0].open, dec!("100"));
    assert_eq!(series.bars()[2].open, dec!("102"));

    // A venue correction for an existing bucket replaces it.
    series.push(bar(1, "101", "105", "100", "104", 500));
    assert_eq!(
        series.len(),
        3,
        "a correction must not duplicate the bucket"
    );
    assert_eq!(series.bars()[1].high, dec!("105"));
}

#[test]
fn the_point_in_time_view_excludes_a_bar_that_has_not_closed() {
    // This is the look-ahead guard: a bar covering [t, t+1m) is only knowable
    // at t+1m, so asking as of t+30s must not reveal it.
    let mut series = BarSeries::new();
    for i in 0..5 {
        series.push(bar(i, "100", "101", "99", "100.5", 100));
    }
    assert_eq!(series.as_of(now()).len(), 0, "nothing has closed yet");
    assert_eq!(
        series
            .as_of(now().saturating_add(Duration::from_secs(30)))
            .len(),
        0
    );
    assert_eq!(
        series
            .as_of(now().saturating_add(Duration::from_mins(1)))
            .len(),
        1
    );
    assert_eq!(
        series
            .as_of(now().saturating_add(Duration::from_mins(3)))
            .len(),
        3
    );
    assert_eq!(
        series
            .as_of(now().saturating_add(Duration::from_hours(1)))
            .len(),
        5
    );
}

#[test]
fn resampling_aggregates_into_coarser_buckets() {
    let mut series = BarSeries::new();
    for i in 0..10 {
        series.push(bar(i, "100", "101", "99", "100.5", 100));
    }
    let five = series.resample(Interval::FiveMinutes);
    assert_eq!(five.len(), 2);
    assert_eq!(five.bars()[0].volume, Decimal::from_int(500));
    assert_eq!(five.bars()[0].interval, Interval::FiveMinutes);
}

#[test]
fn gaps_are_reported_not_silently_filled() {
    let mut series = BarSeries::new();
    series.push(bar(0, "100", "101", "99", "100.5", 100));
    series.push(bar(1, "100", "101", "99", "100.5", 100));
    series.push(bar(5, "100", "101", "99", "100.5", 100));
    let gaps = series.missing_buckets();
    assert_eq!(gaps.len(), 3, "minutes 2, 3 and 4 are missing");
    assert_eq!(series.len(), 3, "the series is not padded");
}

#[test]
fn annualised_volatility_scales_by_the_interval() {
    let mut daily = BarSeries::new();
    let mut price = 100.0;
    for i in 0..252 {
        let shock = if i % 2 == 0 { 1.01 } else { 0.99 };
        let open = price;
        price *= shock;
        daily.push(Bar {
            interval: Interval::Day,
            open_time: now().saturating_add(Duration::from_days(i)),
            open: Decimal::from_f64(open).unwrap(),
            high: Decimal::from_f64(open.max(price) * 1.001).unwrap(),
            low: Decimal::from_f64(open.min(price) * 0.999).unwrap(),
            close: Decimal::from_f64(price).unwrap(),
            ..bar(0, "100", "101", "99", "100.5", 1000)
        });
    }
    let vol = daily.annualised_volatility();
    // A 1% daily alternation annualises to roughly 16%.
    assert!((0.12..0.20).contains(&vol), "annualised vol {vol}");
}

// --- corporate actions ------------------------------------------------------

#[test]
fn a_split_halves_prior_prices_so_the_series_stays_continuous() {
    let ex = now().saturating_add(Duration::from_days(5));
    let action = CorporateAction {
        object_id: id("obj-aapl"),
        ex_date: ex,
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::Split {
            ratio: Decimal::from_int(2),
        },
        announced_at: now(),
    };
    assert_eq!(action.price_adjustment_factor(dec!("200")), Ok(dec!("0.5")));
    assert_eq!(action.quantity_adjustment_factor(), Decimal::from_int(2));

    let prices = vec![
        (now(), dec!("200")),
        (now().saturating_add(Duration::from_days(4)), dec!("210")),
        (ex, dec!("105")),
    ];
    let adjusted = adjust_prices(&prices, &[action]).expect("a two-for-one split is priceable");
    assert_eq!(adjusted[0].1, dec!("100"));
    assert_eq!(adjusted[1].1, dec!("105"));
    assert_eq!(
        adjusted[2].1,
        dec!("105"),
        "post-split prices are untouched"
    );
}

#[test]
fn a_cash_dividend_adjusts_by_the_yield_it_paid() {
    let action = CorporateAction {
        object_id: id("obj-x"),
        ex_date: now(),
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::CashDividend { amount: dec!("2") },
        announced_at: now(),
    };
    // A 2.00 dividend from a 100.00 price scales prior prices by 0.98.
    assert_eq!(
        action.price_adjustment_factor(dec!("100")),
        Ok(dec!("0.98"))
    );
    assert_eq!(action.cash_per_share(), dec!("2"));
    assert_eq!(action.quantity_adjustment_factor(), Decimal::ONE);
}

#[test]
fn structural_actions_do_not_rewrite_history() {
    for kind in [
        CorporateActionKind::Delisting {
            reason: "acquired".into(),
        },
        CorporateActionKind::Renamed {
            new_symbol: "NEW".into(),
        },
        CorporateActionKind::Merger {
            acquirer: "ACME".into(),
            cash_per_share: dec!("50"),
            share_ratio: Decimal::ZERO,
        },
    ] {
        let terminal = matches!(
            kind,
            CorporateActionKind::Delisting { .. } | CorporateActionKind::Merger { .. }
        );
        let action = CorporateAction {
            object_id: id("obj-x"),
            ex_date: now(),
            record_date: None,
            payment_date: None,
            kind,
            announced_at: now(),
        };
        assert_eq!(
            action.price_adjustment_factor(dec!("100")),
            Ok(Decimal::ONE)
        );
        assert_eq!(action.is_terminal(), terminal);
    }
}

#[test]
fn several_actions_compound_in_the_right_order() {
    let split_date = now().saturating_add(Duration::from_days(10));
    let dividend_date = now().saturating_add(Duration::from_days(5));
    let actions = vec![
        CorporateAction {
            object_id: id("obj-x"),
            ex_date: split_date,
            record_date: None,
            payment_date: None,
            kind: CorporateActionKind::Split {
                ratio: Decimal::from_int(2),
            },
            announced_at: now(),
        },
        CorporateAction {
            object_id: id("obj-x"),
            ex_date: dividend_date,
            record_date: None,
            payment_date: None,
            kind: CorporateActionKind::CashDividend { amount: dec!("1") },
            announced_at: now(),
        },
    ];
    let prices = vec![
        (now(), dec!("100")),
        (dividend_date, dec!("99")),
        (split_date, dec!("50")),
    ];
    let adjusted = adjust_prices(&prices, &actions).expect("both actions are priceable");
    // The earliest price absorbs both the split and the dividend.
    assert!(adjusted[0].1 < dec!("50"), "got {}", adjusted[0].1);
    assert_eq!(adjusted[2].1, dec!("50"), "the latest price is unchanged");
}

fn spinoff(value_fraction: f64) -> CorporateAction {
    CorporateAction {
        object_id: id("obj-x"),
        ex_date: now().saturating_add(Duration::from_days(5)),
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::Spinoff {
            spun_entity: "NEWCO".into(),
            value_fraction,
        },
        announced_at: now(),
    }
}

#[test]
fn a_spinoff_fraction_outside_the_unit_interval_is_refused_rather_than_clamped() {
    // `value_fraction.clamp(0.0, 1.0)` is the clamp the core-Rust rules
    // prohibit by name, and both ends of it did harm: 4.0 became 1.0, whose
    // factor of zero erases the entire prior series rather than adjusting it,
    // and `NaN` survived the clamp untouched, failed `Decimal::from_f64` and
    // landed on the shared `unwrap_or(Decimal::ONE)` — no adjustment at all,
    // which is indistinguishable from the honest answer a merger gives.
    for fraction in [f64::NAN, f64::INFINITY, 4.0, 1.0, -0.5] {
        let Err(refusal) = spinoff(fraction).price_adjustment_factor(dec!("100")) else {
            panic!("a fraction of {fraction} is not a share of value and must not be priced");
        };
        assert_eq!(refusal.code(), "invalid", "at {fraction}: {refusal}");
        assert!(
            refusal.message().contains("NEWCO"),
            "the refusal must name the spun entity so the record can be found, at \
             {fraction}: {refusal}"
        );
    }

    // The admitting half: a quarter of the value leaving is an ordinary
    // spinoff, it is priced at 0.75, and the series moves by it. Without this
    // the refusals above would be satisfied by a function that refused
    // everything.
    let action = spinoff(0.25);
    assert_eq!(
        action.price_adjustment_factor(dec!("100")),
        Ok(dec!("0.75"))
    );
    let prices = vec![(now(), dec!("100")), (action.ex_date, dec!("75"))];
    let adjusted = adjust_prices(&prices, &[action]).expect("a quarter is a share of value");
    assert_eq!(
        adjusted[0].1,
        dec!("75"),
        "the prior price loses the spun value"
    );
    assert_eq!(adjusted[1].1, dec!("75"), "the ex-date price is untouched");
}

#[test]
fn an_action_that_cannot_be_priced_is_refused_rather_than_reported_as_no_adjustment() {
    // Every arm used to fall through to `Decimal::ONE`, which `adjust_prices`
    // then skipped — so a corrupt record and a merger produced the same
    // outcome, and the split stayed in the series. An unadjusted split reads
    // as a crash, and every volatility, drawdown and return taken from that
    // series inherits it.
    let cases: Vec<(&str, CorporateActionKind, Decimal)> = vec![
        (
            "a split into zero shares",
            CorporateActionKind::Split {
                ratio: Decimal::ZERO,
            },
            dec!("100"),
        ),
        (
            "a stock dividend that pays away the whole holding",
            CorporateActionKind::StockDividend {
                ratio: Decimal::from_int(-1),
            },
            dec!("100"),
        ),
        (
            "a dividend larger than the price it was paid from",
            CorporateActionKind::CashDividend {
                amount: dec!("150"),
            },
            dec!("100"),
        ),
        (
            "a rights issue against no reference price",
            CorporateActionKind::RightsIssue {
                ratio: Decimal::ONE,
                price: dec!("80"),
            },
            Decimal::ZERO,
        ),
    ];
    for (described, kind, reference) in cases {
        let action = CorporateAction {
            object_id: id("obj-x"),
            ex_date: now(),
            record_date: None,
            payment_date: None,
            kind,
            announced_at: now(),
        };
        let refusal = action
            .price_adjustment_factor(reference)
            .expect_err(described);
        assert_eq!(refusal.code(), "invalid", "{described}: {refusal}");
        assert!(
            refusal.message().contains("obj-x"),
            "{described}: the refusal must name the instrument, got {refusal}"
        );
    }

    // The admitting half, one legitimate value per refused arm, so that what
    // separates the two is the figure and not the arm.
    for (described, kind, reference, expected) in [
        (
            "a two-for-one split",
            CorporateActionKind::Split {
                ratio: Decimal::from_int(2),
            },
            dec!("100"),
            dec!("0.5"),
        ),
        (
            "a one-for-ten stock dividend",
            CorporateActionKind::StockDividend { ratio: dec!("0.1") },
            dec!("100"),
            dec!("0.909090909"),
        ),
        (
            "a two per cent dividend",
            CorporateActionKind::CashDividend { amount: dec!("2") },
            dec!("100"),
            dec!("0.98"),
        ),
        (
            "a one-for-one rights issue at 80",
            CorporateActionKind::RightsIssue {
                ratio: Decimal::ONE,
                price: dec!("80"),
            },
            dec!("100"),
            dec!("0.9"),
        ),
    ] {
        let action = CorporateAction {
            object_id: id("obj-x"),
            ex_date: now(),
            record_date: None,
            payment_date: None,
            kind,
            announced_at: now(),
        };
        assert_eq!(
            action.price_adjustment_factor(reference),
            Ok(expected),
            "{described} must still be priced"
        );
    }

    // And the one thing `Decimal::ONE` is still allowed to mean: a rename
    // leaves the history as traded. Refusal and "no adjustment" are now two
    // answers rather than one.
    let renamed = CorporateAction {
        object_id: id("obj-x"),
        ex_date: now(),
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::Renamed {
            new_symbol: "NEW".into(),
        },
        announced_at: now(),
    };
    assert_eq!(
        renamed.price_adjustment_factor(dec!("100")),
        Ok(Decimal::ONE)
    );
}

// --- curves -----------------------------------------------------------------

fn treasury_curve() -> TermStructure {
    TermStructure::new(
        "UST",
        Currency::USD,
        now(),
        vec![
            CurvePoint {
                tenor_years: 0.25,
                value: 0.0525,
            },
            CurvePoint {
                tenor_years: 2.0,
                value: 0.0450,
            },
            CurvePoint {
                tenor_years: 5.0,
                value: 0.0420,
            },
            CurvePoint {
                tenor_years: 10.0,
                value: 0.0435,
            },
            CurvePoint {
                tenor_years: 30.0,
                value: 0.0460,
            },
        ],
    )
    .unwrap()
}

#[test]
fn the_curve_passes_through_its_quoted_tenors() {
    let curve = treasury_curve();
    assert!(approx_eq(curve.rate_at(2.0), 0.0450, 1e-12));
    assert!(approx_eq(curve.rate_at(10.0), 0.0435, 1e-12));
    // Flat extrapolation beyond the quoted range.
    assert!(approx_eq(curve.rate_at(50.0), 0.0460, 1e-12));
    assert!(approx_eq(curve.rate_at(0.01), 0.0525, 1e-12));
}

#[test]
fn interpolation_never_leaves_the_bracketing_quotes() {
    let curve = treasury_curve();
    for i in 0..=100 {
        let t = 2.0 + (5.0 - 2.0) * f64::from(i) / 100.0;
        let r = curve.rate_at(t);
        assert!(
            (0.0420 - 1e-9..=0.0450 + 1e-9).contains(&r),
            "rate {r} at tenor {t} left the bracketing quotes"
        );
    }
}

#[test]
fn curve_slope_and_inversion_are_detected() {
    let curve = treasury_curve();
    // 3m at 5.25% against 10y at 4.35% is an inverted front end.
    assert!(curve.is_inverted());
    assert!(curve.slope_bps(0.25, 10.0) < 0.0);
    assert!(
        curve.slope_bps(5.0, 30.0) > 0.0,
        "the long end is upward sloping"
    );

    let upward = TermStructure::new(
        "UP",
        Currency::USD,
        now(),
        vec![
            CurvePoint {
                tenor_years: 1.0,
                value: 0.02,
            },
            CurvePoint {
                tenor_years: 10.0,
                value: 0.04,
            },
        ],
    )
    .unwrap();
    assert!(!upward.is_inverted());
}

#[test]
fn forward_rates_and_discount_factors_are_consistent() {
    let curve = treasury_curve();
    let forward = curve.forward_rate(2.0, 5.0).unwrap();
    // Forward from a curve that rises between 5y and 10y must exceed the spot.
    let long_forward = curve.forward_rate(5.0, 10.0).unwrap();
    assert!(long_forward > curve.rate_at(5.0));
    assert!(forward.is_finite());

    assert!(
        curve.forward_rate(5.0, 2.0).is_none(),
        "backwards range is undefined"
    );
    assert!(approx_eq(curve.discount_factor(0.0), 1.0, 1e-12));
    assert!(curve.discount_factor(10.0) < 1.0);
}

#[test]
fn curve_shocks_move_the_whole_structure() {
    let curve = treasury_curve();
    let shifted = curve.shifted(100.0).unwrap();
    assert!(approx_eq(
        shifted.rate_at(10.0),
        curve.rate_at(10.0) + 0.01,
        1e-12
    ));

    let steepened = curve.rotated(2.0, 50.0).unwrap();
    assert!(steepened.slope_bps(2.0, 30.0) > curve.slope_bps(2.0, 30.0));
    assert!(
        approx_eq(steepened.rate_at(2.0), curve.rate_at(2.0), 1e-9),
        "the pivot is fixed"
    );
}

#[test]
fn an_empty_curve_is_rejected() {
    assert!(TermStructure::new("X", Currency::USD, now(), Vec::new()).is_err());
}

#[test]
fn a_curve_with_a_duplicated_maturity_is_refused_naming_the_tenor() {
    // The failure this prevents, which was live in this file: `new` used to
    // `dedup_by` any tenor within 1e-12 of its neighbour, so a vendor
    // publishing the 10y point twice at two different yields had one silently
    // dropped — and which one survived depended on a sort that is not stable
    // across the two orders the same file can arrive in. A curve that
    // interpolates differently on a replay than it did live is not a replay.
    let points = vec![
        CurvePoint {
            tenor_years: 2.0,
            value: 0.0450,
        },
        CurvePoint {
            tenor_years: 10.0,
            value: 0.0435,
        },
        CurvePoint {
            tenor_years: 10.0,
            value: 0.0461,
        },
    ];
    // The premise: the same three points with the duplicate resolved do build
    // a curve, so what is refused below is the duplication and not the shape.
    let mut resolved = points.clone();
    resolved[2].tenor_years = 30.0;
    assert!(
        TermStructure::new("UST", Currency::USD, now(), resolved).is_ok(),
        "the fixture is refused for some other reason, so this proves nothing"
    );

    let refusal = TermStructure::new("UST", Currency::USD, now(), points)
        .expect_err("a maturity quoted twice was accepted");
    let message = refusal.to_string();
    // Match the whole clause, not a bare "10": every yield in the fixture
    // contains a digit that would satisfy a looser assertion.
    assert!(
        message.contains("tenor 10 is quoted twice"),
        "the refusal does not name the duplicated tenor: {message}"
    );
    assert!(
        message.contains("0.0435") && message.contains("0.0461"),
        "the refusal does not name both quoted values: {message}"
    );
}

#[test]
fn a_curve_with_a_negative_tenor_is_refused_rather_than_re_anchored() {
    // A point before the curve's own as-of instant has no meaning, and the
    // monotone interpolator accepted it as the new front end — silently
    // re-anchoring flat extrapolation onto a rate nobody quoted.
    let good = vec![
        CurvePoint {
            tenor_years: 1.0,
            value: 0.02,
        },
        CurvePoint {
            tenor_years: 5.0,
            value: 0.03,
        },
    ];
    assert!(
        TermStructure::new("X", Currency::USD, now(), good.clone()).is_ok(),
        "the premise fails: the curve without the bad point is already refused"
    );

    let mut bad = good;
    bad.push(CurvePoint {
        tenor_years: -0.5,
        value: 0.09,
    });
    let refusal =
        TermStructure::new("X", Currency::USD, now(), bad).expect_err("a negative tenor was taken");
    assert!(
        refusal.to_string().contains("tenor -0.5 is negative"),
        "the refusal does not name the tenor: {refusal}"
    );
}

#[test]
fn a_curve_point_that_is_not_a_finite_number_is_refused() {
    // NaN compared `Equal` under the sort, so it landed wherever the input
    // happened to put it and poisoned every interpolated rate downstream.
    let anchor = CurvePoint {
        tenor_years: 1.0,
        value: 0.02,
    };
    assert!(
        TermStructure::new("X", Currency::USD, now(), vec![anchor]).is_ok(),
        "the premise fails: the anchor alone does not build a curve"
    );

    let nan_tenor = TermStructure::new(
        "X",
        Currency::USD,
        now(),
        vec![
            anchor,
            CurvePoint {
                tenor_years: f64::NAN,
                value: 0.03,
            },
        ],
    )
    .expect_err("a NaN tenor was taken");
    assert!(
        nan_tenor
            .to_string()
            .contains("is not a finite number of years"),
        "the refusal does not say what is wrong: {nan_tenor}"
    );

    let nan_value = TermStructure::new(
        "X",
        Currency::USD,
        now(),
        vec![
            anchor,
            CurvePoint {
                tenor_years: 5.0,
                value: f64::INFINITY,
            },
        ],
    )
    .expect_err("an infinite rate was taken");
    assert!(
        nan_value.to_string().contains("is not a finite rate"),
        "the refusal does not say what is wrong: {nan_value}"
    );
}

#[test]
fn a_present_value_is_the_amount_scaled_by_the_curves_own_discount_factor() {
    // The money/statistics crossing: the rate and the factor are `f64`, the
    // answer is `Decimal`. The property is that the two agree — a
    // `present_value` computed off a different rate than `rate_at` reports
    // would be a valuation nobody could reproduce from the published curve.
    let curve = treasury_curve();
    let factor = curve.discount_factor(10.0);
    // The premise: ten years of discounting actually moves the number, so an
    // implementation returning the amount unchanged would not pass.
    assert!(
        factor < 0.95,
        "the fixture curve barely discounts at all ({factor}), so this proves nothing"
    );

    let present = curve
        .present_value(dec!("1000000"), 10.0)
        .expect("a positive tenor on a well-formed curve");
    let expected = Decimal::from_f64(factor).expect("a discount factor between zero and one")
        * dec!("1000000");
    assert_eq!(
        present, expected,
        "the present value does not match the curve's own discount factor"
    );
    assert!(present < dec!("1000000"), "discounting did not reduce it");

    // Zero tenor is the identity, not a refusal: a cashflow due now is worth
    // its face.
    assert_eq!(
        curve
            .present_value(dec!("1000000"), 0.0)
            .expect("a zero tenor is discountable"),
        dec!("1000000")
    );
}

#[test]
fn discounting_to_a_tenor_that_has_already_passed_is_refused() {
    // Refuse rather than clamp to zero: a negative tenor reaching here is a
    // caller that computed a time to maturity from a stale clock, and a
    // present value returned for it would flow into a valuation as though a
    // matured claim were still outstanding.
    let curve = treasury_curve();
    assert!(
        curve.present_value(dec!("100"), 1.0).is_ok(),
        "the premise fails: the curve discounts nothing at all"
    );
    let refusal = curve
        .present_value(dec!("100"), -1.0)
        .expect_err("a past tenor was discounted");
    assert!(
        refusal.to_string().contains("already received is booked"),
        "the refusal does not say what to do instead: {refusal}"
    );
    assert!(
        curve.present_value(dec!("100"), f64::NAN).is_err(),
        "a non-finite tenor was discounted"
    );
}

#[test]
fn a_yield_quoted_in_percent_is_refused_rather_than_discounting_a_claim_to_zero() {
    // The failure this prevents, and it was live in this file: a vendor
    // quoting `yield_to_maturity` as 4.35 rather than 0.0435 built a perfectly
    // valid curve, and `present_value` returned exactly 0 for every amount at
    // every tenor — because `Decimal::from_f64` rounds to nine decimal places
    // rather than failing, so `exp(-4.35 * 10) = 1.9e-19` arrived as
    // `Decimal::ZERO`. Zero is not a small valuation, it is the absence of
    // one, and a credit register printing "worst claim at 0 of discounted
    // expected loss" reads as a universe carrying no credit risk.
    let percent_quoted = TermStructure::new(
        "UST-percent",
        Currency::USD,
        now(),
        vec![
            CurvePoint {
                tenor_years: 2.0,
                value: 4.50,
            },
            CurvePoint {
                tenor_years: 10.0,
                value: 4.35,
            },
        ],
    )
    .expect("the curve itself is well-formed; it is the unit that is wrong");

    // The premise, asserted before the refusal: the arithmetic really does
    // collapse. Without this the test would pass on an implementation that
    // refused every present value for some unrelated reason.
    let factor = percent_quoted.discount_factor(10.0);
    assert!(
        factor > 0.0 && factor < 1e-15,
        "the fixture does not underflow ({factor}), so this proves nothing"
    );
    assert_eq!(
        Decimal::from_f64(factor),
        Some(Decimal::ZERO),
        "the premise fails: the factor no longer rounds to zero, so the \
         manufactured-zero path this guards does not exist"
    );

    let refusal = percent_quoted
        .present_value(dec!("1000000"), 10.0)
        .expect_err("a million was discounted to zero and returned as a valuation");
    let message = refusal.to_string();
    assert!(
        message.contains("rounds to zero at the scale money is held at"),
        "the refusal does not name what went wrong: {message}"
    );
    assert!(
        message.contains("quoted in percent rather than as a fraction"),
        "the refusal does not say what to do instead: {message}"
    );

    // And the other half of a working gate: the same curve in the right unit
    // is admitted and discounts to a real number. A guard that refused both
    // would be indistinguishable from one that refused everything.
    let fraction_quoted = TermStructure::new(
        "UST-fraction",
        Currency::USD,
        now(),
        vec![
            CurvePoint {
                tenor_years: 2.0,
                value: 0.0450,
            },
            CurvePoint {
                tenor_years: 10.0,
                value: 0.0435,
            },
        ],
    )
    .expect("a curve quoted as fractions");
    let present = fraction_quoted
        .present_value(dec!("1000000"), 10.0)
        .expect("a curve at 4.35% must still discount");
    assert!(
        present > dec!("600000") && present < dec!("700000"),
        "a million discounted ten years at 4.35% is not {present}"
    );
}

#[test]
fn a_stored_curve_is_refitted_from_its_own_points_rather_than_read_by_a_second_method() {
    // The failure this prevents: the interpolator is `serde(skip)`, because a
    // fitted spline is derived state and storing it would give a replay a
    // second source of truth. While the field was an `Option`, a curve read
    // back from the log carried `None` and answered every query by *nearest
    // neighbour* — a different interpolation from the monotone cubic the live
    // curve used, on the same points. A curve that interpolates differently on
    // a replay than it did live is not a replay.
    let live = treasury_curve();
    // The premise: the two methods actually disagree somewhere, so a
    // round-trip that silently switched would be detectable at all. Halfway
    // between the 5y and 10y knots, nearest-neighbour returns one endpoint and
    // the cubic does not.
    let midpoint = 7.5;
    let cubic = live.rate_at(midpoint);
    assert!(
        (cubic - 0.0420).abs() > 1e-6 && (cubic - 0.0435).abs() > 1e-6,
        "the fixture's midpoint {cubic} coincides with a knot, so a switch of \
         method would not show"
    );

    let json = serde_json::to_string(&live).expect("a curve serialises");
    let replayed: TermStructure = serde_json::from_str(&json).expect("a stored curve loads");
    assert!(
        approx_eq(replayed.rate_at(midpoint), cubic, 1e-12),
        "the replayed curve reads {} where the live one read {cubic}",
        replayed.rate_at(midpoint)
    );

    // And a stored payload that would not form a curve is refused at load,
    // rather than becoming a curve with no points that answers every tenor
    // with a rate of zero — a discount factor of one, an amount returned
    // undiscounted as a present value. Built by emptying the points of a
    // payload that has just been proven to load, so the refusal is about the
    // points and not about a field name this test guessed wrong.
    let mut payload: serde_json::Value = serde_json::from_str(&json).expect("the payload parses");
    assert!(
        payload
            .get("points")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|points| points.len() == 5),
        "the premise fails: the stored shape has no `points` array to empty: {payload}"
    );
    payload["points"] = serde_json::Value::Array(Vec::new());
    let empty = serde_json::from_value::<TermStructure>(payload);
    assert!(
        empty.is_err(),
        "a stored curve with no points was accepted and will answer every query"
    );
}

#[test]
fn the_curve_publishes_the_tenors_it_was_actually_quoted_at() {
    // Published so a caller that must not accept flat extrapolation can tell
    // an observed rate from an extended one. Without it `rate_at(40.0)` on a
    // single 10y point returns the 10y rate and is indistinguishable from an
    // observation.
    let curve = treasury_curve();
    let (shortest, longest) = curve.tenor_range();
    assert!(
        approx_eq(shortest, 0.25, 1e-12),
        "the front end is {shortest}"
    );
    assert!(approx_eq(longest, 30.0, 1e-12), "the long end is {longest}");
    // The premise that makes the range worth publishing: outside it the curve
    // answers anyway, with the endpoint value.
    assert!(approx_eq(
        curve.rate_at(longest + 20.0),
        curve.rate_at(longest),
        1e-12
    ));
}

// --- microstructure ---------------------------------------------------------

#[test]
fn microstructure_metrics_decompose_the_spread() {
    let q = quote("100.00", "100.10", 500, 500);
    let quotes = vec![q.clone()];
    // Three buyer-initiated trades at the offer.
    let trades: Vec<Trade> = (0..3)
        .map(|i| Trade {
            object_id: id("obj-aapl"),
            venue: "XNYS".into(),
            at: now().saturating_add(Duration::from_secs(i)),
            price: dec!("100.10"),
            size: Decimal::from_int(100),
            aggressor: Some(Side::Buy),
            condition: TradeCondition::Regular,
            trade_id: Some(format!("t{i}")),
            quality: Default::default(),
        })
        .collect();
    // The mid drifts up afterwards: part of the spread was lost to information.
    let future_mids = vec![100.07, 100.08, 100.09];

    let metrics = MicrostructureMetrics::compute(&quotes, &trades, &future_mids);
    assert_eq!(metrics.trade_count, 3);
    assert_eq!(metrics.total_volume, Decimal::from_int(300));
    assert!(approx_eq(metrics.buy_initiated_fraction, 1.0, 1e-12));
    assert!(approx_eq(metrics.order_flow_imbalance, 1.0, 1e-12));
    assert!(metrics.effective_spread_bps > 0.0);
    // Effective = realised + impact, by construction.
    assert!(
        approx_eq(
            metrics.effective_spread_bps,
            metrics.realised_spread_bps + metrics.price_impact_bps,
            1e-6
        ),
        "effective {} != realised {} + impact {}",
        metrics.effective_spread_bps,
        metrics.realised_spread_bps,
        metrics.price_impact_bps
    );
    assert!(
        metrics.price_impact_bps > 0.0,
        "buying pressure should move the mid up"
    );
}

#[test]
fn non_price_forming_trades_are_excluded() {
    let quotes = vec![quote("100.00", "100.10", 500, 500)];
    let trades = vec![Trade {
        object_id: id("obj-aapl"),
        venue: "XNYS".into(),
        at: now(),
        price: dec!("95.00"),
        size: Decimal::from_int(100_000),
        aggressor: Some(Side::Sell),
        condition: TradeCondition::LateReport,
        trade_id: None,
        quality: Default::default(),
    }];
    let metrics = MicrostructureMetrics::compute(&quotes, &trades, &[]);
    assert_eq!(
        metrics.trade_count, 0,
        "a late print must not drive discovery"
    );
    assert_eq!(metrics.total_volume, Decimal::ZERO);
}

#[test]
fn stress_is_detected_from_spread_and_imbalance() {
    let calm = MicrostructureMetrics {
        quoted_spread_bps: 2.0,
        order_flow_imbalance: 0.1,
        price_impact_bps: 1.0,
        ..Default::default()
    };
    assert!(!calm.is_stressed(2.0));

    let wide = MicrostructureMetrics {
        quoted_spread_bps: 20.0,
        ..calm.clone()
    };
    assert!(wide.is_stressed(2.0));

    let one_sided = MicrostructureMetrics {
        order_flow_imbalance: -0.95,
        ..calm
    };
    assert!(one_sided.is_stressed(2.0));
}

// --- snapshot ---------------------------------------------------------------

#[test]
fn the_snapshot_tracks_the_latest_state_and_advances_its_clock() {
    let mut snapshot = MarketSnapshot::new(now());
    snapshot.apply_quote(quote("100.00", "100.10", 100, 100));
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot.as_of, now());

    let later = now().saturating_add(Duration::from_mins(5));
    snapshot.apply_trade(Trade {
        object_id: id("obj-aapl"),
        venue: "XNYS".into(),
        at: later,
        price: dec!("100.05"),
        size: Decimal::from_int(250),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    });
    assert_eq!(
        snapshot.as_of, later,
        "the snapshot clock follows the events"
    );

    let state = snapshot.get(&id("obj-aapl")).unwrap();
    // A transacted price is preferred over an indicative mid.
    assert_eq!(state.reference_price(), Some(dec!("100.05")));
    assert_eq!(state.session_volume, Decimal::from_int(250));
}

#[test]
fn stale_instruments_are_surfaced() {
    let mut snapshot = MarketSnapshot::new(now());
    snapshot.apply_quote(quote("100.00", "100.10", 100, 100));

    let mut fresh = quote("50.00", "50.10", 100, 100);
    fresh.object_id = id("obj-msft");
    fresh.at = now().saturating_add(Duration::from_hours(2));
    snapshot.apply_quote(fresh);

    let stale = snapshot.stale_instruments(Duration::from_mins(30));
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].0, "obj-aapl");
    assert!(stale[0].1 >= Duration::from_hours(2));
}

#[test]
fn rolling_the_session_clears_volume_but_keeps_prices() {
    let mut snapshot = MarketSnapshot::new(now());
    snapshot.apply_trade(Trade {
        object_id: id("obj-aapl"),
        venue: "XNYS".into(),
        at: now(),
        price: dec!("100"),
        size: Decimal::from_int(1000),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    });
    snapshot.roll_session();
    let state = snapshot.get(&id("obj-aapl")).unwrap();
    assert_eq!(state.session_volume, Decimal::ZERO);
    assert_eq!(
        state.reference_price(),
        Some(dec!("100")),
        "the last price survives"
    );
}
