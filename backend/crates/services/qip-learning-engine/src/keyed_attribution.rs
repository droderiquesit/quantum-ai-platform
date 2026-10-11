//! Outcome attribution keyed by who and what produced each position
//! (blueprint §22, R12-MODEL-6003; §22.1, R12-MODEL-6021).
//!
//! [`crate::attribution::Attribution`] says *where* a position's P&L came
//! from — market move, factor, commission, spread, impact. It joined to
//! nothing but the hypothesis, so the question §22 actually asks — which
//! strategy, which model, which evidence source, which capital policy, which
//! risk rule and which execution path should be credited or blamed — had no
//! answer. A platform that cannot say which component earned a result cannot
//! learn which component to keep.
//!
//! This module joins an attribution to the seven keys stamped on each
//! position **at the instant it was released**, and groups the exact P&L by
//! any one of them. Two commitments:
//!
//! * **Every grouping closes.** A position informed by three models splits its
//!   total across them with [`crate::attribution::split_pro_rata`], so the
//!   per-model figures sum to the book's attributed P&L to the last unit. A
//!   forecaster's attributed P&L is therefore the sum over the positions it
//!   informed (R12-MODEL-6021's check) and not an estimate of it.
//! * **A position nobody stamped is named, not dropped.** A fill whose order
//!   was released before this process started, or by a path that stamped no
//!   keys, carries [`UNATTRIBUTED`] on every dimension and is counted in
//!   [`KeyedAttribution::unkeyed`]. Dropping it would make every grouping sum
//!   to less than the book, which is the residual the attribution refuses to
//!   carry, moved one level up where nobody checks it.

use crate::attribution::{Attribution, split_pro_rata};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The key every dimension carries for a position nobody stamped.
pub const UNATTRIBUTED: &str = "unattributed";

/// The seven things §22 attributes an outcome to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    /// The strategy the position was taken under.
    Strategy,
    /// Each forecaster — detector or analyst — whose view informed it.
    Model,
    /// Each hypothesis (belief) the position expressed.
    Belief,
    /// Each evidence origin behind those hypotheses.
    EvidenceSource,
    /// The capital policy the sizing ran under.
    CapitalPolicy,
    /// The risk rule set that admitted the order, and whether it resized it.
    RiskRule,
    /// Where and how the order was executed.
    ExecutionPath,
}

impl Dimension {
    /// Every dimension, in a fixed order a report can rely on.
    pub const ALL: [Self; 7] = [
        Self::Strategy,
        Self::Model,
        Self::Belief,
        Self::EvidenceSource,
        Self::CapitalPolicy,
        Self::RiskRule,
        Self::ExecutionPath,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Strategy => "strategy",
            Self::Model => "model",
            Self::Belief => "belief",
            Self::EvidenceSource => "evidence_source",
            Self::CapitalPolicy => "capital_policy",
            Self::RiskRule => "risk_rule",
            Self::ExecutionPath => "execution_path",
        }
    }
}

/// The seven keys stamped on one position at release.
///
/// Fields are private and the constructor refuses a blank: a key with no
/// name pools every unnamed position into one bucket, and a report would
/// then credit "" with whatever nobody could explain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributionKeys {
    strategy: String,
    models: Vec<String>,
    beliefs: Vec<String>,
    evidence_sources: Vec<String>,
    capital_policy: String,
    risk_rule: String,
    execution_path: String,
}

impl AttributionKeys {
    /// Stamp a position. The three list dimensions are deduplicated and
    /// sorted, so two stampings of the same facts compare equal and a split
    /// is deterministic; an empty list is stamped [`UNATTRIBUTED`] rather
    /// than left empty, because an empty list would drop the position from
    /// that grouping's total.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        strategy: impl Into<String>,
        models: Vec<String>,
        beliefs: Vec<String>,
        evidence_sources: Vec<String>,
        capital_policy: impl Into<String>,
        risk_rule: impl Into<String>,
        execution_path: impl Into<String>,
    ) -> Result<Self> {
        let strategy = scalar(Dimension::Strategy, strategy.into())?;
        let capital_policy = scalar(Dimension::CapitalPolicy, capital_policy.into())?;
        let risk_rule = scalar(Dimension::RiskRule, risk_rule.into())?;
        let execution_path = scalar(Dimension::ExecutionPath, execution_path.into())?;
        Ok(Self {
            strategy,
            models: list(Dimension::Model, models)?,
            beliefs: list(Dimension::Belief, beliefs)?,
            evidence_sources: list(Dimension::EvidenceSource, evidence_sources)?,
            capital_policy,
            risk_rule,
            execution_path,
        })
    }

    /// The keys of a position nobody stamped: [`UNATTRIBUTED`] everywhere.
    pub fn unattributed() -> Self {
        Self {
            strategy: UNATTRIBUTED.to_string(),
            models: vec![UNATTRIBUTED.to_string()],
            beliefs: vec![UNATTRIBUTED.to_string()],
            evidence_sources: vec![UNATTRIBUTED.to_string()],
            capital_policy: UNATTRIBUTED.to_string(),
            risk_rule: UNATTRIBUTED.to_string(),
            execution_path: UNATTRIBUTED.to_string(),
        }
    }

    /// The values this position carries on `dimension`. Never empty.
    pub fn values(&self, dimension: Dimension) -> Vec<&str> {
        match dimension {
            Dimension::Strategy => vec![self.strategy.as_str()],
            Dimension::Model => self.models.iter().map(String::as_str).collect(),
            Dimension::Belief => self.beliefs.iter().map(String::as_str).collect(),
            Dimension::EvidenceSource => self.evidence_sources.iter().map(String::as_str).collect(),
            Dimension::CapitalPolicy => vec![self.capital_policy.as_str()],
            Dimension::RiskRule => vec![self.risk_rule.as_str()],
            Dimension::ExecutionPath => vec![self.execution_path.as_str()],
        }
    }
}

fn scalar(dimension: Dimension, value: String) -> Result<String> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!(
            "an attribution key for {} is blank; stamp {UNATTRIBUTED:?} if it is genuinely \
             unknown, so the position is counted rather than pooled under an empty name",
            dimension.as_str()
        )));
    }
    Ok(value)
}

fn list(dimension: Dimension, values: Vec<String>) -> Result<Vec<String>> {
    let mut set = BTreeSet::new();
    for value in values {
        set.insert(scalar(dimension, value)?);
    }
    if set.is_empty() {
        set.insert(UNATTRIBUTED.to_string());
    }
    Ok(set.into_iter().collect())
}

/// One position's exact total and the keys it was stamped with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyedPosition {
    pub object_id: String,
    pub total: Decimal,
    pub keys: AttributionKeys,
    /// Whether the keys were stamped at release (`true`) or are the
    /// [`UNATTRIBUTED`] stand-in (`false`).
    pub stamped: bool,
}

/// An attribution joined to its keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyedAttribution {
    pub positions: Vec<KeyedPosition>,
    /// The attribution's own total, which every grouping must sum to.
    pub total: Decimal,
}

impl KeyedAttribution {
    /// Join `attribution` to `keys`, which is keyed by the position's
    /// `object_id` (the order the fills belong to).
    ///
    /// Refuses an attribution that does not itself reconcile: grouping a
    /// residual by strategy would hand the unexplained part to whoever
    /// happened to be first.
    pub fn join(
        attribution: &Attribution,
        keys: &BTreeMap<String, AttributionKeys>,
    ) -> Result<Self> {
        attribution.validate()?;
        let positions = attribution
            .positions
            .iter()
            .map(|position| match keys.get(&position.object_id) {
                Some(stamped) => KeyedPosition {
                    object_id: position.object_id.clone(),
                    total: position.total,
                    keys: stamped.clone(),
                    stamped: true,
                },
                None => KeyedPosition {
                    object_id: position.object_id.clone(),
                    total: position.total,
                    keys: AttributionKeys::unattributed(),
                    stamped: false,
                },
            })
            .collect::<Vec<_>>();
        let total = positions
            .iter()
            .map(|position| position.total)
            .fold(Decimal::ZERO, |a, b| a + b);
        Ok(Self { positions, total })
    }

    /// Positions that carry the [`UNATTRIBUTED`] stand-in.
    pub fn unkeyed(&self) -> usize {
        self.positions
            .iter()
            .filter(|position| !position.stamped)
            .count()
    }

    /// The exact P&L by one dimension's key.
    ///
    /// A position carrying several values on the dimension splits its total
    /// evenly across them by [`split_pro_rata`] — even, because no weighting
    /// was recorded at release and inventing one here would make the figure
    /// a guess; exact, so the map sums to [`Self::total`] to the last unit.
    /// The sum is asserted before returning.
    pub fn by(&self, dimension: Dimension) -> Result<BTreeMap<String, Decimal>> {
        let mut totals: BTreeMap<String, Decimal> = BTreeMap::new();
        for position in &self.positions {
            let values = position.keys.values(dimension);
            let weights = vec![Decimal::from_int(1); values.len()];
            let shares = split_pro_rata(position.total, &weights)?;
            for (value, share) in values.into_iter().zip(shares) {
                *totals.entry(value.to_string()).or_insert(Decimal::ZERO) += share;
            }
        }
        let sum: Decimal = totals.values().copied().sum();
        if sum != self.total {
            return Err(Error::numeric(format!(
                "the attribution by {} sums to {sum} against a book of {}; a grouping that does \
                 not close credits somebody with P&L nobody earned",
                dimension.as_str(),
                self.total
            )));
        }
        Ok(totals)
    }

    /// Every dimension's grouping at once, in [`Dimension::ALL`] order.
    pub fn all(&self) -> Result<BTreeMap<Dimension, BTreeMap<String, Decimal>>> {
        Dimension::ALL
            .iter()
            .map(|dimension| Ok((*dimension, self.by(*dimension)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use crate::attribution::PositionAttribution;
    use qip_core::time::Timestamp;

    fn dec(text: &str) -> Decimal {
        Decimal::parse(text).expect("a decimal literal")
    }

    fn position(id: &str, total: &str) -> PositionAttribution {
        PositionAttribution {
            object_id: id.to_string(),
            hypotheses: vec![format!("hyp-{id}")],
            components: BTreeMap::from([("idiosyncratic".to_string(), dec(total))]),
            total: dec(total),
            average_notional: dec("1000"),
        }
    }

    fn attribution(positions: Vec<PositionAttribution>) -> Attribution {
        let total = positions.iter().map(|p| p.total).sum();
        Attribution {
            from: Timestamp::from_secs(0),
            to: Timestamp::from_secs(0),
            total_pnl: total,
            opening_equity: dec("100000"),
            positions,
            by_source: BTreeMap::new(),
        }
    }

    fn keys(models: &[&str], path: &str) -> AttributionKeys {
        AttributionKeys::new(
            "desk:platform",
            models.iter().map(|m| m.to_string()).collect(),
            vec!["hyp".to_string()],
            vec!["origin-a".to_string()],
            "survival(hedge=0)",
            "kernel-test",
            path,
        )
        .expect("valid keys")
    }

    /// R12-MODEL-6021's check: a forecaster's attributed P&L equals the sum
    /// over the positions it informed. Mutation: splitting with a plain
    /// division instead of `split_pro_rata` loses the unit a third cannot
    /// carry and fails the closure assertion; dropping the multi-model split
    /// (crediting only the first model) fails the per-model figure.
    #[test]
    fn a_forecasters_attributed_pnl_is_the_exact_sum_over_the_positions_it_informed() -> Result<()>
    {
        let attribution = attribution(vec![position("o1", "100"), position("o2", "-40")]);
        let stamped = BTreeMap::from([
            (
                "o1".to_string(),
                keys(&["analyst:a", "analyst:b", "detector:x"], "desk/sim"),
            ),
            ("o2".to_string(), keys(&["analyst:a"], "desk/sim")),
        ]);
        let keyed = KeyedAttribution::join(&attribution, &stamped)?;
        assert_eq!(keyed.unkeyed(), 0, "premise: every position was stamped");
        let by_model = keyed.by(Dimension::Model)?;
        // analyst:a informed both: its first-share of o1 carries the
        // leftover unit, plus all of o2.
        assert_eq!(by_model["analyst:a"], dec("33.333333334") + dec("-40"));
        assert_eq!(by_model["analyst:b"], dec("33.333333333"));
        assert_eq!(by_model["detector:x"], dec("33.333333333"));
        let sum: Decimal = by_model.values().copied().sum();
        assert_eq!(sum, dec("60"), "the per-model figures do not close");
        Ok(())
    }

    /// R12-MODEL-6003's check: every position carries all seven keys, and a
    /// grouping exists for each. Mutation: removing any arm of
    /// `AttributionKeys::values` (or a variant from `Dimension::ALL`) fails
    /// the seven-dimension assertion.
    #[test]
    fn every_dimension_groups_the_same_book_and_each_closes() -> Result<()> {
        let attribution = attribution(vec![position("o1", "75"), position("o2", "25")]);
        let stamped = BTreeMap::from([
            ("o1".to_string(), keys(&["analyst:a"], "desk/sim/XNYS")),
            ("o2".to_string(), keys(&["analyst:b"], "desk/sim/XLON")),
        ]);
        let keyed = KeyedAttribution::join(&attribution, &stamped)?;
        let all = keyed.all()?;
        assert_eq!(all.len(), 7, "a dimension is missing from the report");
        for (dimension, grouping) in &all {
            assert!(!grouping.is_empty(), "{} is empty", dimension.as_str());
            let sum: Decimal = grouping.values().copied().sum();
            assert_eq!(sum, dec("100"), "{} does not close", dimension.as_str());
        }
        assert_eq!(all[&Dimension::ExecutionPath]["desk/sim/XNYS"], dec("75"));
        assert_eq!(all[&Dimension::ExecutionPath]["desk/sim/XLON"], dec("25"));
        Ok(())
    }

    /// Mutation: dropping unstamped positions from `join` makes the book
    /// fail to close and `unkeyed` read zero.
    #[test]
    fn a_position_nobody_stamped_is_counted_under_unattributed_rather_than_dropped() -> Result<()> {
        let attribution = attribution(vec![position("o1", "10"), position("o2", "5")]);
        let stamped = BTreeMap::from([("o1".to_string(), keys(&["analyst:a"], "desk/sim"))]);
        let keyed = KeyedAttribution::join(&attribution, &stamped)?;
        assert_eq!(keyed.unkeyed(), 1);
        let by_strategy = keyed.by(Dimension::Strategy)?;
        assert_eq!(by_strategy[UNATTRIBUTED], dec("5"));
        assert_eq!(by_strategy["desk:platform"], dec("10"));
        Ok(())
    }

    #[test]
    fn a_blank_key_is_refused_rather_than_pooled() {
        assert!(
            AttributionKeys::new("", vec![], vec![], vec![], "p", "r", "e").is_err(),
            "a blank strategy was accepted"
        );
        assert!(
            AttributionKeys::new("s", vec![" ".to_string()], vec![], vec![], "p", "r", "e")
                .is_err(),
            "a blank model was accepted"
        );
    }
}
