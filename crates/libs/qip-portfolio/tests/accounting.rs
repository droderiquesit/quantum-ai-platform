//! Portfolio accounting. The identities here must hold exactly.

use qip_core::testing::{Property, any_positive_decimal, approx_eq};
use qip_core::{Context, Currency, Decimal, Duration, ObjectId, PortfolioId, Timestamp, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::risk_profile::{FactorExposures, RiskCharacteristics, factors};
use qip_financial::universe::Universe;
use qip_portfolio::exposure::Exposure;
use qip_portfolio::lot::{Lot, LotMethod, close_lots_with};
use qip_portfolio::portfolio::Portfolio;
use qip_portfolio::position::{Position, PositionSide};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn later(days: i64) -> Timestamp {
    now().saturating_add(Duration::from_days(days))
}

fn equity(symbol: &str, price: &str, sector: Sector, country: &str) -> FinancialObject {
    let (context, _clock) = Context::deterministic(now(), 1);
    FinancialObject::builder(
        ObjectId::from_string(format!("OBJ{symbol:0>23}")),
        symbol,
        InstrumentType::CommonStock,
    )
    .name(symbol)
    .venue("XNYS")
    .sector(sector)
    .geography(country)
    .price(Decimal::parse(price).unwrap())
    .liquidity(LiquidityProfile::listed(Decimal::from_int(1_000_000), 3.0))
    .transaction_costs(TransactionCostModel::listed(3.0))
    .risk(RiskCharacteristics {
        annualised_volatility: 0.28,
        beta: 1.1,
        factor_exposures: FactorExposures::new()
            .with(factors::MARKET, 1.1)
            .with(factors::MOMENTUM, 0.4),
        ..RiskCharacteristics::default()
    })
    .provenance(Provenance::synthetic("test", now()))
    .build(now())
    .map(|mut o| {
        o.issuer = Some(format!("{symbol} Inc"));
        let _ = &context;
        o
    })
    .expect("valid object")
}

fn portfolio(capital: i64) -> Portfolio {
    Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        Decimal::from_int(capital),
        now(),
    )
}

// --- lots -------------------------------------------------------------------

#[test]
fn fifo_closes_the_oldest_lot_first() {
    let mut lots = vec![
        Lot::new(Decimal::from_int(100), dec!("50"), now()),
        Lot::new(Decimal::from_int(100), dec!("60"), later(10)),
    ];
    let trades = close_lots_with(
        &mut lots,
        Decimal::from_int(100),
        dec!("70"),
        Decimal::ZERO,
        later(20),
        LotMethod::FirstInFirstOut,
    );
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].open_price, dec!("50"), "the oldest lot");
    assert_eq!(trades[0].realised_pnl(), Decimal::from_int(2000));
    assert_eq!(lots.len(), 1);
    assert_eq!(lots[0].price, dec!("60"), "the newer lot survives");
}

#[test]
fn the_lot_method_changes_the_realised_gain() {
    let build = || {
        vec![
            Lot::new(Decimal::from_int(100), dec!("50"), now()),
            Lot::new(Decimal::from_int(100), dec!("60"), later(10)),
        ]
    };
    let realise = |method: LotMethod| {
        let mut lots = build();
        close_lots_with(
            &mut lots,
            Decimal::from_int(100),
            dec!("70"),
            Decimal::ZERO,
            later(20),
            method,
        )
        .iter()
        .map(|t| t.realised_pnl())
        .sum::<Decimal>()
    };

    assert_eq!(realise(LotMethod::FirstInFirstOut), Decimal::from_int(2000));
    assert_eq!(realise(LotMethod::LastInFirstOut), Decimal::from_int(1000));
    // Highest cost minimises the gain.
    assert_eq!(realise(LotMethod::HighestCost), Decimal::from_int(1000));
    assert_eq!(realise(LotMethod::LowestCost), Decimal::from_int(2000));
}

#[test]
fn a_partial_close_leaves_a_proportionally_reduced_lot() {
    let mut lots = vec![Lot::new(Decimal::from_int(100), dec!("50"), now()).with_costs(dec!("10"))];
    let trades = close_lots_with(
        &mut lots,
        Decimal::from_int(40),
        dec!("60"),
        dec!("4"),
        later(5),
        LotMethod::FirstInFirstOut,
    );
    assert_eq!(trades[0].quantity, Decimal::from_int(40));
    // Costs are apportioned: 40% of the 10 opening plus the whole 4 closing.
    assert_eq!(trades[0].costs, dec!("8"));
    assert_eq!(lots.len(), 1);
    assert_eq!(lots[0].quantity, Decimal::from_int(60));
    assert_eq!(
        lots[0].costs,
        dec!("6"),
        "the remaining lot keeps its share of costs"
    );
}

#[test]
fn closing_more_than_is_open_consumes_everything_available() {
    let mut lots = vec![Lot::new(Decimal::from_int(50), dec!("50"), now())];
    let trades = close_lots_with(
        &mut lots,
        Decimal::from_int(200),
        dec!("60"),
        Decimal::ZERO,
        later(1),
        LotMethod::FirstInFirstOut,
    );
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].quantity, Decimal::from_int(50));
    assert!(lots.is_empty());
}

#[test]
fn a_short_lot_realises_a_gain_when_the_price_falls() {
    let mut lots = vec![Lot::new(Decimal::from_int(-100), dec!("50"), now())];
    let trades = close_lots_with(
        &mut lots,
        Decimal::from_int(100),
        dec!("40"),
        Decimal::ZERO,
        later(5),
        LotMethod::FirstInFirstOut,
    );
    assert!(!trades[0].was_long);
    assert_eq!(trades[0].realised_pnl(), Decimal::from_int(1000));
    assert!(trades[0].is_win());
    assert!(approx_eq(trades[0].return_pct(), 0.2, 1e-12));
}

// --- positions --------------------------------------------------------------

#[test]
fn a_position_accumulates_and_averages_correctly() {
    let mut position = Position::new(ObjectId::from_string("obj"), "AAPL", now());
    position.apply_fill(
        Decimal::from_int(100),
        dec!("50"),
        Decimal::ZERO,
        now(),
        None,
    );
    position.apply_fill(
        Decimal::from_int(100),
        dec!("60"),
        Decimal::ZERO,
        later(1),
        None,
    );

    assert_eq!(position.quantity(), Decimal::from_int(200));
    assert_eq!(position.average_price(), dec!("55"));
    assert_eq!(position.side(), PositionSide::Long);
    assert_eq!(position.unrealised_pnl(dec!("70")), Decimal::from_int(3000));
}

#[test]
fn closing_a_position_realises_profit_and_returns_cash() {
    let mut position = Position::new(ObjectId::from_string("obj"), "AAPL", now());
    let bought = position.apply_fill(Decimal::from_int(100), dec!("50"), dec!("5"), now(), None);
    assert_eq!(bought, dec!("-5005"), "cash out is the notional plus costs");

    let sold = position.apply_fill(
        Decimal::from_int(-100),
        dec!("60"),
        dec!("6"),
        later(5),
        None,
    );
    assert_eq!(sold, dec!("5994"), "cash in is the notional less costs");

    assert!(position.is_flat());
    // 1000 gross less 11 of costs.
    assert_eq!(position.realised_pnl, dec!("989"));
    assert_eq!(position.unrealised_pnl(dec!("70")), Decimal::ZERO);
    assert_eq!(position.trade_count(), 1);
    assert!(approx_eq(position.hit_rate(), 1.0, 1e-12));
}

#[test]
fn a_fill_that_exceeds_the_position_flips_it() {
    let mut position = Position::new(ObjectId::from_string("obj"), "AAPL", now());
    position.apply_fill(
        Decimal::from_int(100),
        dec!("50"),
        Decimal::ZERO,
        now(),
        None,
    );
    position.apply_fill(
        Decimal::from_int(-250),
        dec!("60"),
        Decimal::ZERO,
        later(1),
        None,
    );

    assert_eq!(position.quantity(), Decimal::from_int(-150));
    assert_eq!(position.side(), PositionSide::Short);
    assert_eq!(
        position.realised_pnl,
        Decimal::from_int(1000),
        "the long leg realised"
    );
    assert_eq!(
        position.average_price(),
        dec!("60"),
        "the new short is at the fill price"
    );
}

#[test]
fn a_short_position_profits_when_the_price_falls() {
    let mut position = Position::new(ObjectId::from_string("obj"), "AAPL", now());
    position.apply_fill(
        Decimal::from_int(-100),
        dec!("50"),
        Decimal::ZERO,
        now(),
        None,
    );
    assert_eq!(position.side(), PositionSide::Short);
    assert_eq!(position.unrealised_pnl(dec!("40")), Decimal::from_int(1000));
    assert_eq!(
        position.unrealised_pnl(dec!("60")),
        Decimal::from_int(-1000)
    );
    assert_eq!(position.market_value(dec!("40")), Decimal::from_int(-4000));
    assert_eq!(
        position.notional_exposure(dec!("40")),
        Decimal::from_int(4000)
    );
}

#[test]
fn the_contract_multiplier_scales_value_and_profit() {
    let mut position = Position::new(ObjectId::from_string("obj"), "ES", now())
        .with_multiplier(Decimal::from_int(50));
    position.apply_fill(
        Decimal::from_int(2),
        dec!("5000"),
        Decimal::ZERO,
        now(),
        None,
    );

    assert_eq!(
        position.market_value(dec!("5000")),
        Decimal::from_int(500_000)
    );
    assert_eq!(
        position.unrealised_pnl(dec!("5010")),
        Decimal::from_int(1000)
    );

    position.apply_fill(
        Decimal::from_int(-2),
        dec!("5010"),
        Decimal::ZERO,
        later(1),
        None,
    );
    assert_eq!(position.realised_pnl, Decimal::from_int(1000));
}

#[test]
fn a_split_adjusts_lots_without_changing_value() {
    let mut position = Position::new(ObjectId::from_string("obj"), "AAPL", now());
    position.apply_fill(
        Decimal::from_int(100),
        dec!("200"),
        Decimal::ZERO,
        now(),
        None,
    );
    let before = position.market_value(dec!("200"));

    // Two-for-one: twice the shares at half the price.
    position.apply_adjustment(Decimal::from_int(2), dec!("0.5"));
    assert_eq!(position.quantity(), Decimal::from_int(200));
    assert_eq!(position.average_price(), dec!("100"));
    assert_eq!(
        position.market_value(dec!("100")),
        before,
        "value is unchanged"
    );
    assert_eq!(position.unrealised_pnl(dec!("100")), Decimal::ZERO);
}

// --- the portfolio ----------------------------------------------------------

#[test]
fn equity_equals_cash_plus_positions_exactly() {
    let mut book = portfolio(1_000_000);
    let aapl = equity("AAPL", "150", Sector::InformationTechnology, "US");
    let msft = equity("MSFT", "400", Sector::InformationTechnology, "US");

    book.apply_fill(
        &aapl,
        Decimal::from_int(1000),
        dec!("150"),
        dec!("45"),
        now(),
        None,
    );
    book.apply_fill(
        &msft,
        Decimal::from_int(-200),
        dec!("400"),
        dec!("24"),
        now(),
        None,
    );

    let prices = BTreeMap::from([
        (aapl.object_id.as_str().to_string(), dec!("155")),
        (msft.object_id.as_str().to_string(), dec!("390")),
    ]);
    let valuation = book.value(&prices, later(1));

    assert!(valuation.is_balanced(), "{valuation:?}");
    book.verify_accounting(&prices, later(1))
        .expect("books must balance");

    // Cash: 1,000,000 - 150,000 - 45 + 80,000 - 24.
    assert_eq!(valuation.cash, dec!("929931"));
    // Positions: 1000 * 155 - 200 * 390.
    assert_eq!(valuation.position_value, dec!("77000"));
    assert_eq!(valuation.equity, dec!("1006931"));
    // Unrealised: +5000 on the long and +2000 on the short, less the 69 of
    // entry costs still attributed to the open lots.
    assert_eq!(valuation.unrealised_pnl, dec!("6931"));
    // Which is exactly the change in equity, since nothing has been realised.
    assert_eq!(
        valuation.equity - Decimal::from_int(1_000_000),
        valuation.total_pnl(),
        "realised plus unrealised must equal the change in equity"
    );
    assert_eq!(valuation.gross_exposure, dec!("233000"));
    assert_eq!(valuation.net_exposure, dec!("77000"));
}

#[test]
fn property_the_accounting_identity_survives_any_sequence_of_fills() {
    // The identity must hold to the last unit after arbitrary trading, because
    // a book that drifts cannot be reconciled against a broker statement.
    Property::new("accounting identity").cases(200).for_all(
        |rng| {
            use qip_core::rng::Rng;
            let fills: Vec<(i64, Decimal)> = (0..12)
                .map(|_| {
                    let quantity = rng.below(2001) as i64 - 1000;
                    (
                        quantity,
                        any_positive_decimal(rng).round_dp(2).max(dec!("0.01")),
                    )
                })
                .collect();
            fills
        },
        |fills| {
            let mut book = portfolio(1_000_000);
            let object = equity("AAPL", "150", Sector::InformationTechnology, "US");
            for (index, (quantity, price)) in fills.iter().enumerate() {
                if *quantity == 0 {
                    continue;
                }
                book.apply_fill(
                    &object,
                    Decimal::from_int(*quantity),
                    *price,
                    dec!("1.25"),
                    later(index as i64),
                    None,
                );
            }
            let prices = BTreeMap::from([(object.object_id.as_str().to_string(), dec!("175"))]);
            let valuation = book.value(&prices, later(100));
            if !valuation.is_balanced() {
                return Err(format!(
                    "equity {} != cash {} + positions {}",
                    valuation.equity, valuation.cash, valuation.position_value
                ));
            }
            Ok(())
        },
    );
}

#[test]
fn realised_plus_unrealised_equals_the_change_in_equity_net_of_flows() {
    let mut book = portfolio(1_000_000);
    let object = equity("AAPL", "150", Sector::InformationTechnology, "US");

    book.apply_fill(
        &object,
        Decimal::from_int(1000),
        dec!("150"),
        dec!("50"),
        now(),
        None,
    );
    book.apply_fill(
        &object,
        Decimal::from_int(-400),
        dec!("160"),
        dec!("20"),
        later(1),
        None,
    );

    let prices = BTreeMap::from([(object.object_id.as_str().to_string(), dec!("170"))]);
    let valuation = book.value(&prices, later(2));

    // Equity moved from the initial capital by exactly total profit.
    let change = valuation.equity - Decimal::from_int(1_000_000);
    assert_eq!(change, valuation.total_pnl(), "{valuation:?}");
    // Realised: 400 * 10, less the 20 of closing costs and 20 apportioned open.
    assert_eq!(valuation.realised_pnl, dec!("3960"));
}

#[test]
fn an_unpriced_position_is_reported_rather_than_valued_at_zero() {
    // Treating a missing price as worthless understates risk exactly when it
    // matters most.
    let mut book = portfolio(1_000_000);
    let object = equity("ILLIQ", "100", Sector::Industrials, "US");
    book.apply_fill(
        &object,
        Decimal::from_int(500),
        dec!("100"),
        Decimal::ZERO,
        now(),
        None,
    );

    let valuation = book.value(&BTreeMap::new(), later(1));
    assert_eq!(valuation.unpriced, vec!["ILLIQ".to_string()]);
    assert_eq!(valuation.position_value, Decimal::ZERO);
    assert!(
        valuation.is_balanced(),
        "the identity still holds over what was priced"
    );
}

#[test]
fn weights_sum_to_the_invested_fraction_of_equity() {
    let mut book = portfolio(1_000_000);
    let a = equity("AAA", "100", Sector::InformationTechnology, "US");
    let b = equity("BBB", "200", Sector::Financials, "US");
    book.apply_fill(
        &a,
        Decimal::from_int(3000),
        dec!("100"),
        Decimal::ZERO,
        now(),
        None,
    );
    book.apply_fill(
        &b,
        Decimal::from_int(1000),
        dec!("200"),
        Decimal::ZERO,
        now(),
        None,
    );

    let prices = BTreeMap::from([
        (a.object_id.as_str().to_string(), dec!("100")),
        (b.object_id.as_str().to_string(), dec!("200")),
    ]);
    let weights = book.weights(&prices, later(1));
    assert!(approx_eq(weights[a.object_id.as_str()], 0.3, 1e-9));
    assert!(approx_eq(weights[b.object_id.as_str()], 0.2, 1e-9));
    let invested: f64 = weights.values().sum();
    assert!(approx_eq(invested, 0.5, 1e-9), "half invested, half cash");
}

#[test]
fn dividends_are_credited_to_cash() {
    let mut book = portfolio(1_000_000);
    let object = equity("AAPL", "150", Sector::InformationTechnology, "US");
    book.apply_fill(
        &object,
        Decimal::from_int(1000),
        dec!("150"),
        Decimal::ZERO,
        now(),
        None,
    );
    let cash_before = book.cash;

    let credited = book.credit_income(&object.object_id, dec!("0.24"), later(30));
    assert_eq!(credited, Decimal::from_int(240));
    assert_eq!(book.cash, cash_before + Decimal::from_int(240));
}

// --- exposure ---------------------------------------------------------------

#[test]
fn exposures_are_aggregated_along_every_axis() {
    let mut universe = Universe::new();
    let tech = equity("TECH", "100", Sector::InformationTechnology, "US");
    let bank = equity("BANK", "50", Sector::Financials, "GB");
    universe.insert(tech.clone()).unwrap();
    universe.insert(bank.clone()).unwrap();

    let mut book = portfolio(1_000_000);
    book.apply_fill(
        &tech,
        Decimal::from_int(2000),
        dec!("100"),
        Decimal::ZERO,
        now(),
        None,
    );
    book.apply_fill(
        &bank,
        Decimal::from_int(-1000),
        dec!("50"),
        Decimal::ZERO,
        now(),
        None,
    );

    let prices = BTreeMap::from([
        (tech.object_id.as_str().to_string(), dec!("100")),
        (bank.object_id.as_str().to_string(), dec!("50")),
    ]);
    let exposures = book.exposures(&universe, &prices);

    assert_eq!(
        exposures.by_sector.net_of("information_technology"),
        Decimal::from_int(200_000)
    );
    assert_eq!(
        exposures.by_sector.net_of("financials"),
        Decimal::from_int(-50_000)
    );
    assert_eq!(
        exposures.by_country.gross_of("GB"),
        Decimal::from_int(50_000)
    );
    // Gross counts both sides; net offsets them.
    assert_eq!(exposures.gross_exposure(), Decimal::from_int(250_000));
    assert_eq!(exposures.net_exposure(), Decimal::from_int(150_000));
    // The market factor loading applies to both.
    assert!(exposures.by_factor.gross_of(factors::MARKET) > Decimal::ZERO);

    let leverage = exposures.leverage(Decimal::from_int(1_000_000));
    assert!(approx_eq(leverage, 0.25, 1e-9));
}

#[test]
fn concentration_distinguishes_one_big_position_from_several() {
    let mut even = Exposure::new();
    for bucket in ["a", "b", "c", "d"] {
        even.add(bucket, Decimal::from_int(100));
    }
    assert!(approx_eq(even.concentration(), 0.25, 1e-9));
    assert!(approx_eq(even.herfindahl(), 0.25, 1e-9));

    let mut lopsided = Exposure::new();
    lopsided.add("a", Decimal::from_int(700));
    for bucket in ["b", "c", "d"] {
        lopsided.add(bucket, Decimal::from_int(100));
    }
    assert!(approx_eq(lopsided.concentration(), 0.7, 1e-9));
    assert!(lopsided.herfindahl() > even.herfindahl());
    assert_eq!(lopsided.ranked()[0].0, "a");
}

#[test]
fn a_market_neutral_book_is_flat_on_net_and_large_on_gross() {
    let mut exposure = Exposure::new();
    exposure.add("equity", Decimal::from_int(500_000));
    exposure.add("equity", Decimal::from_int(-500_000));
    assert_eq!(exposure.total_net(), Decimal::ZERO);
    assert_eq!(exposure.total_gross(), Decimal::from_int(1_000_000));
}

#[test]
fn an_unclassified_instrument_still_counts_toward_gross() {
    // Dropping it would understate exposure, which is the wrong direction to
    // be wrong in.
    let universe = Universe::new();
    let object = equity("UNKNOWN", "100", Sector::Unclassified, "US");
    let mut book = portfolio(1_000_000);
    book.apply_fill(
        &object,
        Decimal::from_int(1000),
        dec!("100"),
        Decimal::ZERO,
        now(),
        None,
    );

    let prices = BTreeMap::from([(object.object_id.as_str().to_string(), dec!("100"))]);
    let exposures = book.exposures(&universe, &prices);
    assert_eq!(
        exposures.by_asset_class.gross_of("unknown"),
        Decimal::from_int(100_000)
    );
    assert_eq!(exposures.gross_exposure(), Decimal::from_int(100_000));
}

#[test]
fn a_snapshot_captures_the_book() {
    let mut book = portfolio(1_000_000);
    let object = equity("AAPL", "150", Sector::InformationTechnology, "US");
    book.apply_fill(
        &object,
        Decimal::from_int(1000),
        dec!("150"),
        Decimal::ZERO,
        now(),
        None,
    );

    let prices = BTreeMap::from([(object.object_id.as_str().to_string(), dec!("160"))]);
    let snapshot = book.snapshot(&prices, later(1));
    assert_eq!(snapshot.positions["AAPL"], Decimal::from_int(1000));
    assert!(snapshot.valuation.is_balanced());
    assert!(approx_eq(book.total_return(&prices, later(1)), 0.01, 1e-9));
}

#[test]
fn compaction_keeps_positions_with_history() {
    let mut book = portfolio(1_000_000);
    let object = equity("AAPL", "150", Sector::InformationTechnology, "US");
    book.apply_fill(
        &object,
        Decimal::from_int(100),
        dec!("150"),
        Decimal::ZERO,
        now(),
        None,
    );
    book.apply_fill(
        &object,
        Decimal::from_int(-100),
        dec!("160"),
        Decimal::ZERO,
        later(1),
        None,
    );

    assert_eq!(book.position_count(), 0, "flat");
    book.compact();
    assert_eq!(
        book.all_positions().count(),
        1,
        "a closed position keeps its trade history for attribution"
    );
}
