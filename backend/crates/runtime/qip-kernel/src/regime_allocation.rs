//! Blueprint §23.3's production seam: the regime the platform already
//! classifies, read by the sizing that builds a proposal.
//!
//! The classification itself is `Platform::market_regime`, which has run in
//! production since long before this module — for the cost router's
//! intelligence rung. What did not exist was a reader on the allocation side,
//! so the platform could say what regime it was in and size identically in
//! all five. This module is that reader and **nothing else**: it classifies
//! nothing, it holds no history, and it is the only place in the kernel where
//! a regime reaches a weight.
//!
//! # One classifier, one answer
//!
//! [`allocation_regime`] is an exhaustive `match` from
//! `qip_cost_router::MarketRegime` onto
//! `qip_optimization_engine::regime::AllocationRegime`, and the exhaustiveness
//! is the guarantee: a sixth regime arm added to the classifier breaks this
//! build, rather than falling into a default that would size as though
//! nothing had changed. A second classifier here — "the drawdown is deep, so
//! call it a crisis" — would be a second answer to a question the platform
//! has already answered, and the two would disagree on exactly the day it
//! mattered.
//!
//! # It can only narrow
//!
//! [`narrow`] multiplies a cap into the map `Platform::build_proposal`
//! already passes to `PortfolioConstructor::construct_capped`, beside ADR
//! 0063's fill-record cap. Both factors are in `(0, 1]`, so their product is,
//! and a bound can only come down. There is no path from this module to a
//! wider bound, a larger budget or a raised limit, and the favouring half of
//! §23.3's table leaves here as a sentence in the proposal's own compromises
//! carrying no number (`regime::describe`).
//!
//! # `Decimal` and `f64`
//!
//! The caps map is `f64` because a cap is a fraction of a weight bound rather
//! than money — the constructor's bounds are `f64` — and this is the same
//! crossing the fill-record cap makes one line above the call site, where a
//! `Decimal` multiplier becomes `multiplier.to_f64()`. No money passes
//! through this module.

use qip_cost_router::MarketRegime;
use qip_optimization_engine::regime::{self, AllocationRegime};
use qip_portfolio_engine::construction::ApprovedThesis;
use std::collections::{BTreeMap, BTreeSet};

/// The allocation-side name for the regime the cost router's classifier
/// produced.
///
/// Exhaustive on purpose. See the module note.
pub const fn allocation_regime(regime: MarketRegime) -> AllocationRegime {
    match regime {
        MarketRegime::Trending => AllocationRegime::Trending,
        MarketRegime::MeanReverting => AllocationRegime::MeanReverting,
        MarketRegime::Crisis => AllocationRegime::Crisis,
        MarketRegime::Illiquid => AllocationRegime::Illiquid,
        MarketRegime::Quiet => AllocationRegime::Quiet,
    }
}

/// Narrow each thesis's weight bound by the regime its own instrument is in,
/// and say so in `notes`.
///
/// `caps` is the map `construct_capped` takes: object id → multiplier on the
/// mandate's position cap, in `(0, 1]`. An existing entry is **multiplied**
/// rather than replaced, because the two caps are disjoint evidence — ADR
/// 0063's is what this instrument's fills did, this is what the tape is doing
/// — and taking the smaller would discard one of them. The product of two
/// numbers in `(0, 1]` is in `(0, 1]`, so nothing this function writes can be
/// refused by the constructor's own check or widen a bound.
///
/// `regime_of` is `Platform::market_regime`, passed as a closure because the
/// classifier is private to the platform and this module must not grow a
/// second one.
///
/// Nothing is written for a regime whose multiplier is one: an entry of
/// exactly one would be a cap that says nothing, and the proposal's
/// compromises would then name every instrument on every cycle.
pub fn narrow<F>(
    caps: &mut BTreeMap<String, f64>,
    notes: &mut Vec<String>,
    theses: &[ApprovedThesis],
    regime_of: F,
) where
    F: Fn(&str) -> MarketRegime,
{
    // The distinct instruments, in a stable order. One narrowing per
    // instrument per construction, even where two theses name the same
    // object: the regime is a property of the instrument's tape, so applying
    // it once per thesis would square it and size the name against a bound
    // nobody chose. Collecting first rather than tracking a `seen` set while
    // applying, because the second is the shape that produced exactly that
    // bug before `a_second_thesis_on_one_instrument_narrows_that_instrument_once`
    // caught it.
    let objects: BTreeSet<&str> = theses
        .iter()
        .map(|thesis| thesis.object_id.as_str())
        .collect();
    for object in objects {
        let regime = allocation_regime(regime_of(object));
        let multiplier = regime::unattributed_multiplier(regime);
        if multiplier >= 1.0 {
            continue;
        }
        let entry = caps.entry(object.to_string()).or_insert(1.0);
        *entry *= multiplier;
        notes.push(format!("{object}: {}", regime::describe(regime)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::Decimal;
    use qip_core::ids::ObjectId;

    fn thesis(object: &str) -> ApprovedThesis {
        ApprovedThesis {
            hypothesis_id: format!("hyp-{object}"),
            object_id: ObjectId::from_string(object),
            conviction: 0.5,
            expected_return: 0.02,
            price: Decimal::from_int(100),
        }
    }

    #[test]
    fn every_regime_the_classifier_can_return_maps_to_one_the_allocator_reads() {
        // The premise: the classifier's own enumeration, not a list written
        // here. If an arm is added there this loop still compiles only
        // because `allocation_regime` is exhaustive — and if the mapping
        // were ever made lossy, two classifier arms would collapse onto one
        // allocation arm and this assertion would catch it.
        assert_eq!(MarketRegime::ALL.len(), 5, "the classifier's arms moved");
        let mapped: Vec<&str> = MarketRegime::ALL
            .into_iter()
            .map(|regime| allocation_regime(regime).as_str())
            .collect();
        assert_eq!(
            mapped,
            vec!["trending", "mean_reverting", "crisis", "illiquid", "quiet"],
            "a regime lost its name on the way to the allocator"
        );
        for regime in MarketRegime::ALL {
            assert_eq!(
                allocation_regime(regime).as_str(),
                regime.as_str(),
                "the allocation-side name disagrees with the classifier's own"
            );
        }
    }

    #[test]
    fn a_regime_narrows_a_bound_and_compounds_with_the_fill_record_cap_rather_than_replacing_it() {
        // The failure this prevents: taking the smaller of the two caps,
        // which would throw away one instrument's fill evidence whenever the
        // tape happened to be uncertain — and the reverse, replacing the
        // fill cap, which would widen a bound the fill record had narrowed.
        let theses = [thesis("obj-AAA"), thesis("obj-BBB")];
        let mut caps = BTreeMap::new();
        // ADR 0063 already halved AAA on its own fills.
        caps.insert("obj-AAA".to_string(), 0.5);
        let mut notes = Vec::new();
        narrow(&mut caps, &mut notes, &theses, |_| MarketRegime::Crisis);
        assert_eq!(
            caps.get("obj-AAA"),
            Some(&0.25),
            "the regime cap replaced the fill cap instead of compounding with it"
        );
        assert_eq!(caps.get("obj-BBB"), Some(&0.5));
        assert_eq!(notes.len(), 2, "the compromises do not name both names");
        assert!(
            notes.iter().all(|note| note.contains("crisis")),
            "a compromise does not name the regime that caused it: {notes:?}"
        );
        assert!(
            caps.values().all(|cap| *cap > 0.0 && *cap <= 1.0),
            "a cap left the range the constructor accepts: {caps:?}"
        );
    }

    #[test]
    fn a_second_thesis_on_one_instrument_narrows_that_instrument_once() {
        // Two theses, one object: the narrowing is a property of the
        // instrument's tape, so applying it per thesis would square it and
        // size the name against a bound nobody chose.
        let theses = [thesis("obj-AAA"), thesis("obj-AAA")];
        assert_eq!(theses.len(), 2, "the premise is two theses on one object");
        let mut caps = BTreeMap::new();
        let mut notes = Vec::new();
        narrow(&mut caps, &mut notes, &theses, |_| MarketRegime::Quiet);
        assert_eq!(caps.len(), 1);
        assert_eq!(
            caps.get("obj-AAA"),
            Some(&0.5),
            "the narrowing was applied twice to one instrument"
        );
    }

    #[test]
    fn a_directional_regime_narrows_less_than_one_the_platform_cannot_name() {
        // The regime has to be an input to the size, or §23.3 is a control
        // whose output never depends on its input. These are the two values
        // production can take today, and they differ.
        let theses = [thesis("obj-AAA")];
        let mut trending = BTreeMap::new();
        narrow(&mut trending, &mut Vec::new(), &theses, |_| {
            MarketRegime::Trending
        });
        let mut illiquid = BTreeMap::new();
        narrow(&mut illiquid, &mut Vec::new(), &theses, |_| {
            MarketRegime::Illiquid
        });
        let trending = *trending.get("obj-AAA").expect("a trending tape narrows");
        let illiquid = *illiquid.get("obj-AAA").expect("an illiquid tape narrows");
        assert!(
            illiquid < trending,
            "an unnameable regime ({illiquid}) did not narrow harder than a directional one \
             ({trending})"
        );
        assert!(trending < 1.0, "the trending arm narrows nothing at all");
    }
}
