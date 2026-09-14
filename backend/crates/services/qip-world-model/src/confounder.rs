//! Common causes — blueprint §9.1's fifth layer, and the qualifier §9.2 puts
//! on the one establishment method this platform can actually compute.
//!
//! # The failure this exists to prevent
//!
//! [`crate::granger`] establishes an edge from temporal precedence: the
//! cause's past carries information about the effect's future that the
//! effect's own past does not. Run that test over pairs drawn from one book
//! while a single driver moves the whole book — a market factor, a funding
//! rate, an index flow, a sector shock — and the driver reproduces itself as
//! an edge between very many of the pairs it touches. Each edge is
//! significant. Each is spurious. And they are spurious *together*, which is
//! the exact failure the whole of blueprint §9 was written to answer: when
//! the regime breaks, every model that learned the same absent structure
//! breaks at the same moment, and nothing in the system can say which
//! relationship should have survived.
//!
//! An uncontrolled pairwise scan is therefore not a weak version of the
//! method. It is a machine for manufacturing the failure.
//!
//! # The two kinds, and why they are one type with two constructors
//!
//! §9.1 wants confounders "explicit, and adjusted for". §9.4 concedes that
//! "confounders are often unobserved" and says what to do then: "where a
//! plausible unobserved confounder exists it is recorded as such, and the
//! edge is treated as suggestive rather than established". Those are two
//! different obligations and this module keeps them apart:
//!
//! * [`Confounder::observed`] carries a series. It is passed to
//!   [`qip_numerics::stats::granger_causality_controlling_for`] and genuinely
//!   removed from the comparison.
//! * [`Confounder::unobserved`] carries no series and adjusts for nothing.
//!   Naming one is an admission, not a remedy, and the only thing it buys is
//!   that the resulting edge is marked
//!   [`crate::causal::EdgeStanding::Suggestive`] rather than passed off as
//!   established.
//!
//! Recording an unobserved confounder must never read as having handled it.
//! That is why [`ConfounderSet::observed_series`] cannot return one — an
//! unobserved confounder is structurally incapable of reaching the
//! regression — rather than relying on a caller to filter correctly.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// The most confounders one [`ConfounderSet`] may hold.
///
/// Two independent reasons, and either alone would justify a cap.
///
/// Statistical: every observed confounder costs `lag` regressors in both
/// fits, so an unbounded set silently eats the degrees of freedom the F-test
/// needs, and a test with almost no residual degrees of freedom reports
/// confident-looking numbers computed from nothing.
///
/// Operational: this set is built per pass from whatever drivers the
/// platform has series for, and a working set with no bound is an outage
/// waiting for a universe large enough to find it. Bounded working sets are
/// not negotiable here.
pub const MAX_CONFOUNDERS: usize = 8;

/// Whether a named common cause was actually adjusted for, or only admitted
/// to.
///
/// Deliberately not a boolean. `adjusted: false` reads, at a call site, as a
/// flag somebody could flip; `Unobserved` reads as what it is — a confounder
/// the platform cannot measure and has recorded rather than pretended away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfounderStanding {
    /// The platform holds a series for it and it was put into both
    /// regressions.
    Observed,
    /// Plausible, named, and not measurable here. Recorded under §9.4 so the
    /// edge it bears on is marked suggestive.
    Unobserved,
}

impl ConfounderStanding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Unobserved => "unobserved",
        }
    }
}

/// A common cause that would otherwise produce a spurious edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Confounder {
    id: String,
    /// Why this is a plausible common cause of the pairs it is applied to.
    ///
    /// Required, not optional, and refused when blank. A confounder whose
    /// rationale nobody wrote down is indistinguishable a quarter later from
    /// a series somebody added because it was to hand, and the whole value
    /// of the confounders layer is that it is *explicit*.
    rationale: String,
    /// Present exactly when the confounder is observed. See the module doc
    /// for why this is not a public field.
    series: Option<Vec<f64>>,
}

impl Confounder {
    /// A confounder the platform holds a series for, and will adjust for.
    ///
    /// Refuses rather than clamps: an empty series, a non-finite
    /// observation, a blank id, and a blank rationale are each a caller bug
    /// that would otherwise survive into a regression as a column of
    /// something nobody can name.
    pub fn observed(
        id: impl Into<String>,
        rationale: impl Into<String>,
        series: Vec<f64>,
    ) -> Result<Self> {
        let id = id.into();
        let rationale = rationale.into();
        Self::check_labels(&id, &rationale)?;
        if series.is_empty() {
            return Err(Error::invalid(format!(
                "confounder '{id}' was declared observed with an empty series; pass the \
                 observations, or declare it unobserved with `Confounder::unobserved`"
            )));
        }
        if series.iter().any(|v| !v.is_finite()) {
            return Err(Error::invalid(format!(
                "confounder '{id}' has a non-finite observation; fix the series at its source \
                 rather than filtering it here"
            )));
        }
        Ok(Self {
            id,
            rationale,
            series: Some(series),
        })
    }

    /// A confounder that is plausible and cannot be measured here.
    ///
    /// This adjusts for nothing. Its whole effect is that an edge carrying
    /// it is marked [`crate::causal::EdgeStanding::Suggestive`], which is
    /// §9.4's handling — "recorded as such, and the edge is treated as
    /// suggestive rather than established".
    pub fn unobserved(id: impl Into<String>, rationale: impl Into<String>) -> Result<Self> {
        let id = id.into();
        let rationale = rationale.into();
        Self::check_labels(&id, &rationale)?;
        Ok(Self {
            id,
            rationale,
            series: None,
        })
    }

    fn check_labels(id: &str, rationale: &str) -> Result<()> {
        if id.trim().is_empty() {
            return Err(Error::invalid(
                "a confounder needs a non-blank id; it is the name the edge records it under",
            ));
        }
        if rationale.trim().is_empty() {
            return Err(Error::invalid(format!(
                "confounder '{id}' needs a rationale saying why it is a plausible common cause; \
                 an unexplained control is indistinguishable later from a series added because \
                 it was to hand"
            )));
        }
        Ok(())
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn rationale(&self) -> &str {
        &self.rationale
    }

    pub fn standing(&self) -> ConfounderStanding {
        match self.series {
            Some(_) => ConfounderStanding::Observed,
            None => ConfounderStanding::Unobserved,
        }
    }

    /// The observations, or `None` for an unobserved confounder.
    pub fn series(&self) -> Option<&[f64]> {
        self.series.as_deref()
    }
}

/// The confounders under which one causal test is run.
///
/// A [`BTreeMap`] rather than a hash map, and that is load-bearing rather
/// than habit: [`Self::observed_series`] hands the regression its control
/// columns in this order, and the column order changes the arithmetic of a
/// least-squares solve in the last bits. A graph rebuilt from the same
/// evidence in a different order would hold edges with different strengths,
/// and a replay that reorders is not a replay.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfounderSet {
    by_id: BTreeMap<String, Confounder>,
}

impl ConfounderSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a confounder.
    ///
    /// Refuses a duplicate id rather than replacing: two different series
    /// under one name is a caller bug, and silently keeping the second would
    /// make which one was adjusted for depend on call order.
    pub fn add(&mut self, confounder: Confounder) -> Result<()> {
        if self.by_id.len() >= MAX_CONFOUNDERS {
            return Err(Error::invalid(format!(
                "a confounder set holds at most {MAX_CONFOUNDERS}; '{}' would be the {}th. \
                 Drop the weakest common cause rather than raising the cap: each control \
                 spends degrees of freedom the F-test needs",
                confounder.id(),
                self.by_id.len() + 1
            )));
        }
        if self.by_id.contains_key(confounder.id()) {
            return Err(Error::invalid(format!(
                "confounder '{}' is already in this set; two series under one name makes which \
                 was adjusted for depend on call order",
                confounder.id()
            )));
        }
        self.by_id.insert(confounder.id().to_string(), confounder);
        Ok(())
    }

    /// Builder form of [`Self::add`].
    pub fn with(mut self, confounder: Confounder) -> Result<Self> {
        self.add(confounder)?;
        Ok(self)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&Confounder> {
        self.by_id.get(id)
    }

    /// The control columns, in id order.
    ///
    /// An unobserved confounder cannot appear here — it has no series to
    /// appear as. That is the structural half of the module's argument: a
    /// caller cannot accidentally treat an admission as an adjustment,
    /// because the type gives it nothing to pass.
    pub fn observed_series(&self) -> Vec<&[f64]> {
        self.by_id
            .values()
            .filter_map(|confounder| confounder.series())
            .collect()
    }

    /// Ids of the confounders that were genuinely adjusted for.
    pub fn observed_ids(&self) -> BTreeSet<String> {
        self.by_id
            .values()
            .filter(|c| c.standing() == ConfounderStanding::Observed)
            .map(|c| c.id().to_string())
            .collect()
    }

    /// Ids of the confounders recorded as plausible and unmeasured.
    ///
    /// Non-empty means every edge established under this set is suggestive,
    /// whatever its p-value.
    pub fn unobserved_ids(&self) -> BTreeSet<String> {
        self.by_id
            .values()
            .filter(|c| c.standing() == ConfounderStanding::Unobserved)
            .map(|c| c.id().to_string())
            .collect()
    }

    /// Whether every observed series is as long as `expected`.
    ///
    /// Checked by [`crate::granger`] before it calls the regression, so that
    /// a mis-sampled control is refused by name here rather than as an
    /// anonymous index from the numerics layer.
    pub fn misaligned(&self, expected: usize) -> Option<&str> {
        self.by_id
            .values()
            .find(|c| c.series().is_some_and(|s| s.len() != expected))
            .map(|c| c.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unobserved_confounder_offers_no_series_to_the_regression() {
        // The premise: the set holds something, so an empty answer below is
        // a filter working rather than an empty set.
        let set = ConfounderSet::new()
            .with(Confounder::unobserved("risk_appetite", "moves both books at once").unwrap())
            .unwrap();
        assert_eq!(set.len(), 1, "the premise: the set is not empty");

        // The failure this prevents: an unobserved confounder reaching the
        // control columns would be adjusting for nothing while reporting
        // that it had adjusted.
        assert!(
            set.observed_series().is_empty(),
            "an unobserved confounder has no series and must not reach the regression"
        );
        assert_eq!(set.unobserved_ids().len(), 1);
        assert!(set.observed_ids().is_empty());
    }

    #[test]
    fn a_confounder_without_a_rationale_is_refused_rather_than_stored() {
        let refused = Confounder::observed("factor", "   ", vec![0.1, 0.2]);
        assert!(
            refused.is_err(),
            "a blank rationale is refused, not stored as an empty string"
        );
    }

    #[test]
    fn a_ninth_confounder_is_refused_rather_than_silently_dropped() {
        let mut set = ConfounderSet::new();
        for index in 0..MAX_CONFOUNDERS {
            set.add(
                Confounder::observed(format!("driver-{index}"), "a common cause", vec![0.1, 0.2])
                    .unwrap(),
            )
            .unwrap();
        }
        // The premise: the set is exactly full, so the refusal below is the
        // cap firing rather than an unrelated error.
        assert_eq!(set.len(), MAX_CONFOUNDERS);
        let refused =
            set.add(Confounder::observed("one-too-many", "a common cause", vec![0.1]).unwrap());
        assert!(refused.is_err(), "the cap refuses rather than drops");
        assert_eq!(set.len(), MAX_CONFOUNDERS, "and the set is unchanged");
    }

    #[test]
    fn the_same_confounder_id_twice_is_refused_rather_than_replacing_the_first() {
        let mut set = ConfounderSet::new();
        set.add(Confounder::observed("rates", "discount rate moves both", vec![0.1]).unwrap())
            .unwrap();
        let refused =
            set.add(Confounder::observed("rates", "a different series", vec![0.9]).unwrap());
        assert!(refused.is_err());
        assert_eq!(
            set.get("rates").and_then(|c| c.series()),
            Some(&[0.1][..]),
            "the first series is kept; a replacement would make the adjustment depend on call order"
        );
    }
}
