//! What each analyst reads from the feature store, by the contract's names.
//!
//! Derived from the analysts' own tables rather than restated, so a read an
//! analyst adds is a read this module reports, and the acceptance test that
//! walks every read against the absorb arms sees it. The other direction is
//! the point: a table here that an analyst did not read from would be a
//! contract about nothing, and it cannot exist because there is no table
//! here — only the analysts'.

use crate::analysts::{ALT_METRICS, MACRO_SERIES};
use crate::manifests::ids;
use qip_world_model::vocabulary::{
    CAUSAL_CLAIM_NEEDED, FeatureRead, SubjectKind, names, unwritten,
};

/// The contract the reads are spelled in, re-exported so a composition root
/// that holds the organisation can hold a fixture to the same vocabulary
/// without a dependency edge of its own on the world model.
pub use qip_world_model::vocabulary::{AltMetric, MacroSeries};

/// One feature one analyst reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalystRead {
    /// The analyst's roster id.
    pub agent: &'static str,
    pub read: FeatureRead,
}

impl AnalystRead {
    const fn new(agent: &'static str, name: &'static str, keyed_by: SubjectKind) -> Self {
        Self {
            agent,
            read: FeatureRead::new(name, keyed_by),
        }
    }
}

/// Every feature-store read any analyst makes.
///
/// The credit, derivatives, commodities and FX reads are listed beside their
/// analysts' code rather than in a table those analysts consult, because each
/// reads a handful of names in a shape (a level, a pair of legs, three curve
/// points) that a table would not simplify; the names themselves are still
/// the contract's constants, so the two cannot spell one differently.
pub fn reads() -> Vec<AnalystRead> {
    let mut reads: Vec<AnalystRead> = MACRO_SERIES
        .iter()
        .map(|(series, _)| AnalystRead {
            agent: ids::MACRO,
            read: series.read(),
        })
        .collect();
    reads.extend([
        AnalystRead::new(
            ids::CREDIT,
            names::CREDIT_SPREAD_BPS,
            SubjectKind::Instrument,
        ),
        AnalystRead::new(
            ids::CREDIT,
            names::EFFECTIVE_DURATION,
            SubjectKind::Instrument,
        ),
        AnalystRead::new(
            ids::DERIVATIVES,
            names::IMPLIED_VOLATILITY,
            SubjectKind::Instrument,
        ),
        AnalystRead::new(
            ids::COMMODITIES,
            names::FRONT_MONTH_PRICE,
            SubjectKind::Instrument,
        ),
        AnalystRead::new(
            ids::COMMODITIES,
            names::DEFERRED_MONTH_PRICE,
            SubjectKind::Instrument,
        ),
        AnalystRead::new(
            ids::COMMODITIES,
            names::DEFERRED_TENOR_MONTHS,
            SubjectKind::Instrument,
        ),
        AnalystRead::new(ids::FX_RATES, names::BASE_RATE, SubjectKind::Instrument),
        AnalystRead::new(ids::FX_RATES, names::QUOTE_RATE, SubjectKind::Instrument),
        AnalystRead::new(
            ids::FX_RATES,
            names::REALISED_VOLATILITY,
            SubjectKind::Instrument,
        ),
    ]);
    reads.extend(ALT_METRICS.iter().map(|(metric, _)| AnalystRead {
        agent: ids::ALT_DATA,
        read: metric.read(),
    }));
    reads
}

/// The analysts no absorb arm can feed, each with the record kind that would.
///
/// Derived from [`reads`] and the contract's `UNWRITTEN` table, plus the
/// causal analyst, whose read is the causal graph rather than a feature. One
/// entry per analyst, in roster order, so a report can name them.
pub fn structurally_blind() -> Vec<(&'static str, &'static str)> {
    let mut blind: Vec<(&str, &str)> = Vec::new();
    for read in reads() {
        if let Some(declared) = unwritten(read.read)
            && !blind.iter().any(|(agent, _)| *agent == read.agent)
        {
            blind.push((read.agent, declared.needs));
        }
    }
    blind.push((ids::CAUSAL, CAUSAL_CLAIM_NEEDED));
    blind
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_read_is_by_a_rostered_analyst_and_the_blind_list_names_the_causal_analyst() {
        let roster = crate::manifests::roster(qip_core::Timestamp::from_secs(1_760_000_000));
        let reads = reads();
        assert!(reads.len() >= 6, "premise: {} reads", reads.len());
        for read in &reads {
            assert!(
                roster.get(read.agent).is_some(),
                "{} is not on the roster",
                read.agent
            );
        }
        let blind = structurally_blind();
        assert!(blind.iter().any(|(agent, _)| *agent == ids::CAUSAL));
        // Not the two the arms now feed.
        assert!(!blind.iter().any(|(agent, _)| *agent == ids::MACRO));
        assert!(!blind.iter().any(|(agent, _)| *agent == ids::ALT_DATA));
    }
}
