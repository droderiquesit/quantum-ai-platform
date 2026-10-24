//! What each belief class's error is applied to (R12-AMBIENT-3113, ADR 0158
//! decisions 7-8).
//!
//! `MetaIntelligence::propose_specialists` weights a class's residual error by
//! the money that error touches. The ambient loop used to pass the whole
//! book's equity as every class's exposure, so the weight ranked residuals and
//! said nothing about money: a class with a twelve-point residual and no open
//! thesis outweighed one with a four-point residual on the whole book.
//!
//! A class's exposure here is the gross notional, at cost, of the instruments
//! **only that class** holds an open thesis on. An instrument two classes hold
//! open theses on is *shared*, an instrument held with no open thesis is
//! *unclaimed*, and both go to `unattributed`: counted, never spread. Spreading
//! a shared notional across its classes would double-count it in the sum, and
//! giving it to the first class would let iteration order decide which class
//! earns a specialist. Every position lands in exactly one bucket, so the
//! buckets sum to the gross book at cost, which is the invariant a reader can
//! check.
//!
//! Money is `Decimal`; the cost is the book's average entry price, not a mark.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::Decimal;

/// Exposure by belief class, and what could not be attributed to one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassExposure {
    /// Gross notional at cost of the instruments only this class holds an
    /// open thesis on. A class with no such instrument has no entry, which is
    /// a statement that nothing is attributed to it and not a zero someone
    /// computed.
    pub by_class: BTreeMap<String, Decimal>,
    /// Gross notional at cost of shared instruments (open theses from two or
    /// more classes) and unclaimed ones (a position, no open thesis).
    pub unattributed: Decimal,
}

impl ClassExposure {
    /// `by_class` plus `unattributed`: the gross book at cost.
    pub fn total(&self) -> Decimal {
        self.by_class
            .values()
            .fold(self.unattributed, |sum, notional| sum + *notional)
    }
}

/// Attribute `positions` (signed quantity and average entry price, by
/// instrument) to the classes of `open_theses` (class, instrument).
pub fn attribute<'a>(
    open_theses: impl IntoIterator<Item = (&'a str, &'a str)>,
    positions: &BTreeMap<String, (Decimal, Decimal)>,
) -> ClassExposure {
    let mut classes_on: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (class, instrument) in open_theses {
        classes_on.entry(instrument).or_default().insert(class);
    }
    let mut exposure = ClassExposure::default();
    for (instrument, (quantity, average_price)) in positions {
        let notional = quantity.abs() * *average_price;
        let sole = classes_on
            .get(instrument.as_str())
            .filter(|classes| classes.len() == 1)
            .and_then(|classes| classes.iter().next());
        match sole {
            Some(class) => {
                let held = exposure.by_class.entry((*class).to_string()).or_default();
                *held += notional;
            }
            None => exposure.unattributed += notional,
        }
    }
    exposure
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn book() -> BTreeMap<String, (Decimal, Decimal)> {
        BTreeMap::from([
            ("obj-A".to_string(), (dec!("10"), dec!("100"))),
            ("obj-B".to_string(), (dec!("-5"), dec!("40"))),
            ("obj-C".to_string(), (dec!("2"), dec!("50"))),
            ("obj-D".to_string(), (dec!("1"), dec!("10"))),
        ])
    }

    /// Two classes hold open theses on `obj-B`, one on `obj-A` alone, none on
    /// `obj-C`, and `obj-D` is a short-horizon thesis of `momentum` only. The
    /// failure prevented is a shared notional given to whichever class sorts
    /// first, which would let iteration order decide who earns a specialist.
    ///
    /// Mutation: attribute a shared instrument to the first class that has a
    /// thesis on it — `unattributed` loses the 200 of `obj-B` and the
    /// sum and shared assertions fail.
    #[test]
    fn a_class_that_alone_holds_an_open_thesis_on_an_instrument_owns_its_notional_and_a_shared_instrument_is_unattributed()
     {
        let theses = [
            ("price_move", "obj-A"),
            ("price_move", "obj-B"),
            ("momentum", "obj-B"),
            ("momentum", "obj-D"),
        ];
        let exposure = attribute(theses, &book());
        assert_eq!(
            exposure.by_class.get("price_move"),
            Some(&dec!("1000")),
            "premise: obj-A is price_move's alone: {exposure:?}"
        );
        assert_eq!(
            exposure.by_class.get("momentum"),
            Some(&dec!("10")),
            "obj-D is momentum's alone: {exposure:?}"
        );
        // obj-B (shared, |-5| * 40 = 200) and obj-C (unclaimed, 2 * 50 = 100).
        assert_eq!(
            exposure.unattributed,
            dec!("300"),
            "a shared or unclaimed notional was attributed to a class: {exposure:?}"
        );
        assert_eq!(
            exposure.total(),
            dec!("1310"),
            "the buckets must sum to the gross book at cost"
        );
    }

    /// A class with no open thesis has no entry, and a repeated thesis of one
    /// class on one instrument does not make the instrument shared.
    #[test]
    fn a_class_with_no_open_thesis_has_no_entry_and_a_class_repeating_itself_does_not_share() {
        let exposure = attribute([("price_move", "obj-A"), ("price_move", "obj-A")], &book());
        assert_eq!(exposure.by_class.get("price_move"), Some(&dec!("1000")));
        assert_eq!(exposure.by_class.get("momentum"), None);
        assert!(attribute([], &BTreeMap::new()).by_class.is_empty());
    }
}
