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
use qip_financial::extensions::{BondDetails, CouponFrequency, DayCount, Extension, Seniority};
use qip_financial::object::FinancialObject;
use qip_financial::universe::Universe;
use qip_market_ingestion::synthetic::SyntheticEnvironment;

/// The provenance source stamped on every derived object, so a record traced
/// back from the event log names the module that wrote it.
pub const SOURCE: &str = "qip-deepbrain-synthetic-reference";

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
        let mut builder = FinancialObject::builder(
            instrument.object_id.clone(),
            instrument.symbol.clone(),
            kind,
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
}
