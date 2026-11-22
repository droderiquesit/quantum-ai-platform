//! What each evidence source earned in the decisions that consumed it
//! (R12-DATA-2025).
//!
//! The deep brain's source desk scored a connector on whether its price change
//! anticipated the *own stream's* next price change. That says whether a
//! source leads a price; it does not say whether a decision that used the
//! source made or lost money, and a source can lead a price and still be
//! followed by losing decisions once costs and sizing are paid. LEARN already
//! knows the second thing: every position it attributes is stamped, at
//! release, with the origins of the evidence its hypothesis was formed on
//! ([`crate::attribution::HypothesisLineage::evidence_sources`]), and
//! [`KeyedAttribution::by`] splits each position's exact P&L across them.
//!
//! This module reads that grouping and nothing else, so the figure a source is
//! credited is the figure the journaled attribution record already carries.
//!
//! A position stamped `unattributed` is left out of the result: it names no
//! source, and crediting it would invent a source called `unattributed`.
//! Advisory: nothing here sizes, routes or sends.

use crate::attribution::AttributionPass;
use qip_core::Decimal;
use qip_core::error::Result;
use qip_learning_engine::keyed_attribution::{Dimension, UNATTRIBUTED};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One source's share of one LEARN pass: the P&L of the positions whose
/// decisions consumed it, and how many positions those were.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionContribution {
    /// The exact P&L credited to the source; positions that carry several
    /// sources split evenly, as [`KeyedAttribution::by`] does.
    pub pnl: Decimal,
    /// Positions whose stamped evidence names the source. A source used by no
    /// decision is absent from the map, not present with zero.
    pub positions: usize,
}

/// Every named source the pass's decisions consumed, with what they earned.
pub fn decision_contributions(
    pass: &AttributionPass,
) -> Result<BTreeMap<String, DecisionContribution>> {
    let pnl = pass.keyed.by(Dimension::EvidenceSource)?;
    let mut out = BTreeMap::new();
    for (source, pnl) in pnl {
        if source == UNATTRIBUTED {
            continue;
        }
        let positions = pass
            .keyed
            .positions
            .iter()
            .filter(|position| {
                position
                    .keys
                    .values(Dimension::EvidenceSource)
                    .contains(&source.as_str())
            })
            .count();
        out.insert(source, DecisionContribution { pnl, positions });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_core::Timestamp;
    use qip_learning_engine::attribution::Attribution;
    use qip_learning_engine::keyed_attribution::{AttributionKeys, KeyedAttribution};

    fn keys(sources: &[&str]) -> Result<AttributionKeys> {
        AttributionKeys::new(
            "s",
            vec!["m".into()],
            vec!["h".into()],
            sources.iter().map(|s| (*s).to_string()).collect(),
            "cp",
            "rr",
            "ep",
        )
    }

    fn pass(rows: &[(&str, i64, &[&str])]) -> Result<AttributionPass> {
        // A KeyedAttribution built directly: the pass's other fields are not
        // this module's input.
        let positions = rows
            .iter()
            .map(|(id, total, sources)| {
                Ok(qip_learning_engine::keyed_attribution::KeyedPosition {
                    object_id: (*id).to_string(),
                    total: Decimal::from_int(*total),
                    keys: keys(sources)?,
                    stamped: true,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let total = positions.iter().map(|p| p.total).sum();
        Ok(AttributionPass {
            attribution: Attribution {
                from: Timestamp::from_secs(0),
                to: Timestamp::from_secs(0),
                total_pnl: total,
                opening_equity: Decimal::ZERO,
                positions: Vec::new(),
                by_source: BTreeMap::new(),
            },
            keyed: KeyedAttribution { positions, total },
            unmarked: 0,
            book_total: total,
            held_moves: BTreeMap::new(),
            deployed: BTreeMap::new(),
        })
    }

    /// The direction the requirement asks for: a source consumed only by a
    /// profitable decision is credited a gain, one consumed only by a losing
    /// decision a loss, and a source no decision used is not in the result.
    /// Mutation: have `decision_contributions` return `Decimal::ZERO` for
    /// every pnl — the first two assertions fail.
    #[test]
    fn a_source_is_credited_with_what_the_decisions_that_used_it_earned() -> Result<()> {
        let got =
            decision_contributions(&pass(&[("o1", 100, &["winner"]), ("o2", -40, &["loser"])])?)?;
        assert_eq!(got.len(), 2, "premise: two sources named: {got:?}");
        assert_eq!(got["winner"].pnl, Decimal::from_int(100));
        assert_eq!(got["loser"].pnl, Decimal::from_int(-40));
        assert!(!got.contains_key("idle"));
        Ok(())
    }

    /// Two sources behind one decision split its P&L evenly and the shares
    /// close to the decision's total; both count the position. Mutation:
    /// count `positions` as `1` flat — the three-position source reads one.
    #[test]
    fn a_shared_decision_splits_its_pnl_and_counts_for_each_source() -> Result<()> {
        let got = decision_contributions(&pass(&[
            ("o1", 100, &["a", "b"]),
            ("o2", 10, &["a"]),
            ("o3", 10, &["a"]),
        ])?)?;
        assert_eq!(got["a"].pnl, Decimal::from_int(70));
        assert_eq!(got["b"].pnl, Decimal::from_int(50));
        assert_eq!(got["a"].positions, 3);
        assert_eq!(got["b"].positions, 1);
        Ok(())
    }

    /// A position nobody stamped names no source and is not credited to a
    /// source called `unattributed`.
    #[test]
    fn an_unattributed_position_credits_no_source() -> Result<()> {
        let got = decision_contributions(&pass(&[("o1", 5, &[]), ("o2", 7, &["a"])])?)?;
        assert!(!got.contains_key(UNATTRIBUTED), "{got:?}");
        assert_eq!(got["a"].pnl, Decimal::from_int(7));
        Ok(())
    }
}
