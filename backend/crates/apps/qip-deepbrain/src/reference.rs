//! The research node's reference-data source, derived from its bar source.
//!
//! The gap this closes: the node's `EvolutionEngine` was assembled with
//! `Universe::new()`, because "no reference-data source exists yet". A backtest
//! against an empty universe rejects every order as an unknown instrument,
//! fills nothing, and — since the no-fill refusal landed — discards every
//! candidate. Honest, but the evolution loop was off, and the drift control the
//! learning desk carries measured nothing on a node that never registered.
//!
//! # Why the universe is derived and not catalogued
//!
//! The node's default data source is the synthetic exchange, and the synthetic
//! exchange already owns the authoritative list of its instruments — ids,
//! symbols, venues, prices. Deriving the universe from that list means the
//! reference data and the bars describe one instrument each, from one
//! definition. A committed catalogue alongside it would be a second definition
//! of the same five instruments, and two definitions that drift apart is a
//! defect nobody finds because both look right in isolation.
//!
//! # Why this is not fabricated data
//!
//! Nothing here invents market behaviour. The prices are the environment's own
//! starting prices, the instruments are the ones every bar this node observes
//! is stamped with, and every object carries `Provenance::synthetic` with
//! `LicensingClass::Synthetic` — so nothing downstream can mistake it for a
//! licensed live source, and the licensing gate that refuses research-only
//! sources sees exactly what it is.
//!
//! A replay recording carries no instrument definitions, so a replay
//! deployment still has no reference-data source; that absence stays visible
//! rather than being papered over with objects guessed from bar stamps.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_financial::Provenance;
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::extensions::{BondDetails, CouponFrequency, DayCount, Extension, Seniority};
use qip_financial::object::FinancialObject;
use qip_financial::universe::Universe;
use qip_market_ingestion::synthetic::SyntheticEnvironment;
use qip_market_ingestion::synthetic::book::BookParameters;

/// The provenance source stamped on every derived object, so a record traced
/// back from the event log names the module that wrote it.
pub const SOURCE: &str = "qip-deepbrain-synthetic-reference";

/// Sessions this module states an exit from the thinly traded name takes, and
/// the share of its volume the desk may be while doing it.
///
/// Neither is measured, and that is why they are named here rather than folded
/// into the arithmetic below where they would read as derived. See
/// [`liquidity_of`] for the rule they follow.
const THIN_EXIT_SESSIONS: f64 = 3.0;
const THIN_PARTICIPATION_RATE: f64 = 0.05;

/// What this module says an exit from one synthetic instrument costs.
///
/// Three of the six figures are the environment's own and one is house policy;
/// the remaining two are this module's statement, and the split is the whole
/// point of the function existing separately.
///
/// **Measured.** The book parameters are what the simulator quotes and trades
/// against, so a record derived from them describes the same instrument the
/// bars describe — the argument this module already makes about prices and
/// instrument ids, applied to the one block that used to be exempt from it.
/// `typical_spread_bps` is twice the book's own half-spread at unit volatility
/// in a normal regime, which is what "typical" means: the realised quote
/// widens with volatility and with a stressed regime, so this is the quote the
/// desk sees when nothing is happening rather than an average over conditions
/// nobody has simulated yet. `average_daily_volume` is the session's trade
/// count times its median trade size, and `top_of_book_depth` is the size the
/// book rests at the touch.
///
/// **Stated.** The environment defines no exit horizon at all — it has no
/// notion of a position, so nothing in it can say how long leaving one takes.
/// This module states one, exactly as it already states the synthetic
/// sovereign's coupon, maturity and duration for the same reason. The rule it
/// follows is the committed catalogue's, which
/// `a_committed_record_the_tape_does_not_cover_is_quoted_no_tighter_than_every_record_it_does`
/// enforces there: a figure nobody measured may only tighten a control, never
/// loosen one. So the exchange-listed names — which this venue trades
/// thousands of times a session — take one session at the house participation
/// rate, and the thinly traded one, whose book publishes sixty trades a
/// session, takes [`THIN_EXIT_SESSIONS`] at [`THIN_PARTICIPATION_RATE`]: no
/// faster and no keener than anything beside it. `thin` is the caller's, and
/// the caller reads it off the same venue convention it already reads the
/// instrument type off.
///
/// **Why not simply the most conservative figure available.** A uniform ten
/// sessions — one day's whole volume at the house rate — reads as caution and
/// is not. `Rung::classify` drops anything past one session to
/// `BondsAndLessLiquidListed`, so it would place a name the simulator trades
/// thirty thousand times a session *below* listed equity on the liquidity
/// ladder, and the ladder's per-rung exit rates would then be ordered by a
/// number nobody computed. That is `illiquid`'s hardcoded 250bps with its sign
/// flipped, and it is why the conservative direction is applied within what
/// the venue actually publishes rather than on top of it.
///
/// Fallible because the volume and depth figures are `f64` in the
/// environment's book and a quantity is a `Decimal` here — that is the
/// crossing point. A non-representable one is refused rather than replaced,
/// for the reason the price beside it is: a reference figure nobody set is
/// worse than a start-up failure naming the instrument.
fn liquidity_of(book: &BookParameters, symbol: &str, thin: bool) -> Result<LiquidityProfile> {
    let volume = book.daily_trade_count * book.median_trade_size;
    let average_daily_volume = Decimal::from_f64(volume).ok_or_else(|| {
        Error::numeric(format!(
            "the synthetic book for {symbol} implies an average daily volume of {volume} ({} \
             trades a session at a median size of {}), which is not representable as a decimal; \
             refusing to invent a volume for it",
            book.daily_trade_count, book.median_trade_size
        ))
    })?;
    let top_of_book_depth = Decimal::from_f64(book.base_touch_size).ok_or_else(|| {
        Error::numeric(format!(
            "the synthetic book for {symbol} rests {} at the touch, which is not representable \
             as a decimal; refusing to invent a depth for it",
            book.base_touch_size
        ))
    })?;
    let (days_to_liquidate, max_participation_rate) = if thin {
        (THIN_EXIT_SESSIONS, THIN_PARTICIPATION_RATE)
    } else {
        (1.0, LiquidityProfile::HOUSE_PARTICIPATION_RATE)
    };
    Ok(LiquidityProfile {
        average_daily_volume,
        typical_spread_bps: book.base_half_spread_bps * 2.0,
        top_of_book_depth,
        days_to_liquidate,
        max_participation_rate,
        // The venue matches every one of these on a published book, including
        // the over-the-counter name, and the environment proves it by
        // quoting depth for it. Negotiated would suppress the volume-based
        // exit estimate `days_to_exit` gives, which is the one figure here
        // that is a measurement.
        is_negotiated: false,
    })
}

/// A universe holding the synthetic exchange's own instruments.
///
/// Bounded by construction: the environment's instrument list is fixed at
/// assembly and this reads it once. Refuses an environment with no instruments
/// rather than returning the empty universe it was written to replace — an
/// empty result here would recreate the exact defect silently.
pub fn synthetic_universe(environment: &SyntheticEnvironment, at: Timestamp) -> Result<Universe> {
    if environment.instruments().is_empty() {
        return Err(Error::invalid(
            "the synthetic environment defines no instruments; a universe derived from it \
             would be the empty one the evolution loop refuses every candidate against",
        ));
    }
    let mut universe = Universe::new();
    for instrument in environment.instruments() {
        // The environment does not carry an instrument type; its convention is
        // that the one over-the-counter instrument is its government bond and
        // everything exchange-listed is common stock. Stated here because the
        // contract multiplier the type implies reaches the backtester's P&L.
        let bond = instrument.venue == "OTC";
        let kind = if bond {
            InstrumentType::GovernmentBond
        } else {
            InstrumentType::CommonStock
        };
        // Prices are money, so the crossing from the environment's f64 state
        // to `Decimal` happens here — and refuses a non-representable value
        // rather than substituting one, because a reference price nobody set
        // is worse than a start-up failure naming the instrument.
        let price = Decimal::from_f64(instrument.state.price).ok_or_else(|| {
            Error::numeric(format!(
                "the synthetic price {} of {} is not representable as a decimal; refusing to \
                 invent a reference price for it",
                instrument.state.price, instrument.symbol
            ))
        })?;
        // The same venue convention decides the exit horizon: the one
        // over-the-counter name is the one the environment gives a thin book,
        // and a record that inherited a figure instead would be sized and
        // vetoed on a number this module never wrote down.
        let liquidity = liquidity_of(&instrument.book, &instrument.symbol, bond)?;
        let mut builder = FinancialObject::builder(
            instrument.object_id.clone(),
            instrument.symbol.clone(),
            kind,
            liquidity,
        )
        .venue(instrument.venue.clone())
        .price(price)
        .provenance(Provenance::synthetic(SOURCE, at));
        if bond {
            // The object model refuses a bond with no maturity, which is
            // correct: a fixed-income instrument without one has no duration
            // and no price. The environment defines only the bond's price
            // process, so its terms are stated here as the synthetic
            // sovereign's — a constant-maturity ten-year note whose modified
            // duration matches the 8.2 the demo's price process loads the
            // rates factor with.
            builder = builder.extension(Extension::Bond(BondDetails {
                issuer: "synthetic sovereign".into(),
                coupon_rate: 0.0425,
                coupon_frequency: CouponFrequency::SemiAnnual,
                maturity: at.saturating_add(Duration::from_days(3653)),
                issue_date: at,
                face_value: Decimal::from_int(100),
                day_count: DayCount::ActualActual,
                seniority: Seniority::SeniorUnsecured,
                credit_rating: None,
                yield_to_maturity: 0.0431,
                modified_duration: 8.2,
                convexity: 58.0,
                option_adjusted_spread_bps: 0.0,
                callable: false,
                puttable: false,
                inflation_index: None,
            }));
        }
        let object = builder.build(at)?;
        universe.insert(object)?;
    }
    Ok(universe)
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_market_ingestion::synthetic::EnvironmentConfig;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn the_derived_universe_describes_every_instrument_the_bars_are_stamped_with() -> Result<()> {
        // The property that makes the backtests fill: every object id a bar
        // carries resolves to a reference object at the bar's own venue and
        // price. An id the universe cannot resolve is an order the backtester
        // rejects as unknown, which is the empty-universe defect one
        // instrument at a time.
        let environment = SyntheticEnvironment::demo(at(), EnvironmentConfig::default());
        // The premise: the environment defines instruments at all.
        assert!(
            !environment.instruments().is_empty(),
            "the demo environment defines no instruments, so fidelity is untestable"
        );

        let universe = synthetic_universe(&environment, at())?;
        assert_eq!(
            universe.len(),
            environment.instruments().len(),
            "the universe and the environment disagree about how many instruments exist"
        );
        for instrument in environment.instruments() {
            let object = universe.require(&instrument.object_id)?;
            assert_eq!(object.symbol, instrument.symbol);
            assert_eq!(object.venue, instrument.venue);
            let expected = Decimal::from_f64(instrument.state.price)
                .ok_or_else(|| Error::numeric("an unrepresentable fixture price"))?;
            assert_eq!(
                object.price, expected,
                "the reference price of {} is not the environment's own",
                instrument.symbol
            );
            // Provenance must say what this is. Reference data that could be
            // mistaken for a licensed live source is the thing the licensing
            // gate exists to refuse.
            assert_eq!(object.provenance.source, SOURCE);
        }
        Ok(())
    }

    #[test]
    fn the_over_the_counter_instrument_is_a_bond_with_a_maturity() -> Result<()> {
        // The object model refuses a bond without a maturity, so if the venue
        // convention ever stops holding, this derivation fails at build time
        // rather than registering the bond as an equity — and this test is
        // what notices the convention changing before a deployment does.
        let environment = SyntheticEnvironment::demo(at(), EnvironmentConfig::default());
        let bond = environment
            .instruments()
            .iter()
            .find(|instrument| instrument.venue == "OTC")
            .ok_or_else(|| Error::not_found("the demo's over-the-counter instrument"))?;

        let universe = synthetic_universe(&environment, at())?;
        let object = universe.require(&bond.object_id)?;
        assert_eq!(object.instrument_type, InstrumentType::GovernmentBond);
        match &object.extension {
            Extension::Bond(details) => {
                assert!(
                    details.maturity > at(),
                    "the bond matured before it was issued"
                );
            }
            other => {
                return Err(Error::invalid(format!(
                    "the bond carries no bond terms: {other:?}"
                )));
            }
        }
        Ok(())
    }

    /// Every derived record quotes the book it will actually be traded
    /// against, rather than a figure a constructor supplied.
    ///
    /// The defect this closes: the universe stated no liquidity at all, so
    /// every one of these five records carried `LiquidityProfile::default()` —
    /// a 10bp quote, a one-session exit and no volume — and the deep brain's
    /// backtests were sized, and its risk limits vetoed, against a number that
    /// appeared in neither the environment nor this file. The assertions are
    /// against the environment's own `BookParameters` rather than against
    /// constants, so a record and the bars it is served beside cannot drift
    /// apart without this failing.
    #[test]
    fn every_derived_record_quotes_the_book_the_environment_will_trade_it_on() -> Result<()> {
        let environment = SyntheticEnvironment::demo(at(), EnvironmentConfig::default());
        // The premise: the demo does not give every instrument the same book,
        // so the equalities below are comparisons and not one constant seen
        // five times.
        let spreads: Vec<f64> = environment
            .instruments()
            .iter()
            .map(|instrument| instrument.book.base_half_spread_bps)
            .collect();
        assert!(
            spreads.iter().any(|spread| !approx(*spread, spreads[0])),
            "every instrument in the demo environment quotes the same half-spread, so this \
             test cannot tell a derived figure from a constant"
        );

        let universe = synthetic_universe(&environment, at())?;
        for instrument in environment.instruments() {
            let object = universe.require(&instrument.object_id)?;
            let book = &instrument.book;
            let liquidity = &object.liquidity;
            assert!(
                approx(
                    liquidity.typical_spread_bps,
                    book.base_half_spread_bps * 2.0
                ),
                "{} is quoted at {}bps against a book whose half-spread is {}",
                instrument.symbol,
                liquidity.typical_spread_bps,
                book.base_half_spread_bps
            );
            let expected_volume =
                Decimal::from_f64(book.daily_trade_count * book.median_trade_size)
                    .ok_or_else(|| Error::numeric("an unrepresentable fixture volume"))?;
            assert_eq!(
                liquidity.average_daily_volume, expected_volume,
                "{} states a volume its own book does not trade",
                instrument.symbol
            );
            let expected_depth = Decimal::from_f64(book.base_touch_size)
                .ok_or_else(|| Error::numeric("an unrepresentable fixture depth"))?;
            assert_eq!(
                liquidity.top_of_book_depth, expected_depth,
                "{} states a depth its own book does not rest",
                instrument.symbol
            );
            assert!(
                !liquidity.is_negotiated,
                "{} is matched on a published book and stated as trading by negotiation",
                instrument.symbol
            );
        }
        Ok(())
    }

    /// The exit horizon nobody measured only ever tightens a control.
    ///
    /// The environment defines no notion of a position and so can measure no
    /// exit time; this module states one. The rule it has to keep is the
    /// committed catalogue's — a stated figure exits no faster and
    /// participates no harder than a measured one — and the thin
    /// over-the-counter book is the record that has to prove it, because it is
    /// the one a uniform figure would have flattered.
    #[test]
    fn the_thinly_traded_record_exits_slower_and_participates_less_than_the_listed_ones()
    -> Result<()> {
        let environment = SyntheticEnvironment::demo(at(), EnvironmentConfig::default());
        let universe = synthetic_universe(&environment, at())?;

        let mut listed = Vec::new();
        let mut thin = Vec::new();
        for instrument in environment.instruments() {
            let object = universe.require(&instrument.object_id)?;
            if instrument.venue == "OTC" {
                thin.push(object.liquidity.clone());
            } else {
                listed.push(object.liquidity.clone());
            }
        }
        // The premise: both sides exist, or the comparison below is a filter
        // over an empty list and passes for ever.
        assert!(!listed.is_empty(), "the environment lists nothing");
        assert!(!thin.is_empty(), "the environment has no thin book");

        let slowest_listed = listed
            .iter()
            .map(|profile| profile.days_to_liquidate)
            .fold(f64::NEG_INFINITY, f64::max);
        let keenest_listed = listed
            .iter()
            .map(|profile| profile.max_participation_rate)
            .fold(f64::INFINITY, f64::min);
        assert!(
            slowest_listed.is_finite() && keenest_listed.is_finite(),
            "the listed records state no bound to compare against"
        );
        for profile in &thin {
            assert!(
                profile.days_to_liquidate > slowest_listed,
                "the thin book claims to exit in {} sessions, no slower than the {slowest_listed} \
                 of the slowest name the venue trades continuously",
                profile.days_to_liquidate
            );
            assert!(
                profile.max_participation_rate < keenest_listed,
                "the thin book would be {} of a session's volume, no less than the \
                 {keenest_listed} the desk permits itself in a name that trades continuously",
                profile.max_participation_rate
            );
        }
        Ok(())
    }

    /// Exact enough for figures that are a product and a doubling of `f64`
    /// inputs, and loose enough that the last bit does not decide a test.
    fn approx(left: f64, right: f64) -> bool {
        (left - right).abs() <= 1e-9 * right.abs().max(1.0)
    }
}
