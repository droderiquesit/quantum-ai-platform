//! Finding something to take over from a source that died or drifted.
//!
//! The failure this module exists to prevent is a silent downgrade. A feed
//! stops, a search returns the least-bad thing available, and the platform
//! carries on with a source covering two thirds of the instruments at a
//! quarter of the frequency — with nothing anywhere saying so. Every
//! downstream number stays plausible.
//!
//! So a partial match is never returned as a replacement. It is returned in a
//! separate variant, next to the gap it leaves, and a caller that wants to
//! accept one has to look at what is missing to reach it.

use crate::coverage::{CoverageGap, CoverageMatch, SourceCoverage};
use crate::scoring::SourceScores;
use serde::{Deserialize, Serialize};

/// A candidate that covers everything the lost source did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankedReplacement {
    source_id: String,
    coverage: CoverageMatch,
    scores: SourceScores,
}

impl RankedReplacement {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn coverage(&self) -> &CoverageMatch {
        &self.coverage
    }

    pub fn scores(&self) -> &SourceScores {
        &self.scores
    }

    /// Ranking key: coverage completeness first, then the composite score.
    ///
    /// Coverage leads because a fuller replacement that scores slightly worse
    /// still answers the question that was asked, and a better-scoring one
    /// that covers less answers a different question.
    pub fn rank_key(&self) -> (f64, f64) {
        (self.coverage.completeness(), self.scores.composite())
    }
}

/// A candidate that does not cover everything, and what it misses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialCandidate {
    source_id: String,
    coverage: CoverageMatch,
    scores: SourceScores,
}

impl PartialCandidate {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn coverage(&self) -> &CoverageMatch {
        &self.coverage
    }

    pub fn scores(&self) -> &SourceScores {
        &self.scores
    }

    /// Exactly what this candidate would not deliver.
    pub fn gap(&self) -> &CoverageGap {
        &self.coverage.gap
    }

    pub fn describe(&self) -> String {
        format!(
            "`{}` covers {:.0}% and would still leave: {}",
            self.source_id,
            self.coverage.completeness() * 100.0,
            self.coverage.gap.describe()
        )
    }
}

/// What a replacement search found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "replacement", rename_all = "snake_case")]
pub enum ReplacementOutcome {
    /// One or more candidates cover the lost source completely, best first.
    Found { ranked: Vec<RankedReplacement> },
    /// Nothing covers it. The closest partial candidates are listed with
    /// their gaps, explicitly *not* as replacements.
    NotFound {
        considered: usize,
        /// The union of what no candidate could supply.
        uncovered: CoverageGap,
        closest: Vec<PartialCandidate>,
    },
}

impl ReplacementOutcome {
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found { .. })
    }

    /// The best complete replacement, or `None`. There is deliberately no
    /// method that returns a partial candidate as if it were a replacement.
    pub fn best(&self) -> Option<&RankedReplacement> {
        match self {
            Self::Found { ranked } => ranked.first(),
            Self::NotFound { .. } => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Found { ranked } => format!(
                "{} candidate(s) cover the lost source; best is `{}`",
                ranked.len(),
                ranked
                    .first()
                    .map(RankedReplacement::source_id)
                    .unwrap_or("none")
            ),
            Self::NotFound {
                considered,
                uncovered,
                closest,
            } => {
                let partials = if closest.is_empty() {
                    "no partial candidate came close".to_string()
                } else {
                    closest
                        .iter()
                        .map(PartialCandidate::describe)
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                format!(
                    "none of the {considered} candidate(s) covers this source. Uncovered: {}. \
                     {partials}",
                    uncovered.describe()
                )
            }
        }
    }
}

/// Rank candidates against the coverage a lost source had.
///
/// `candidates` are `(id, coverage, scores)`. Deterministic: complete matches
/// are ordered by coverage then composite score then id, so an identical
/// candidate set always produces an identical ranking.
pub fn search(
    required: &SourceCoverage,
    candidates: &[(String, SourceCoverage, SourceScores)],
) -> ReplacementOutcome {
    let mut complete: Vec<RankedReplacement> = Vec::new();
    let mut partial: Vec<PartialCandidate> = Vec::new();

    for (id, coverage, scores) in candidates {
        let against = coverage.against(required);
        if against.is_complete() {
            complete.push(RankedReplacement {
                source_id: id.clone(),
                coverage: against,
                scores: *scores,
            });
        } else {
            partial.push(PartialCandidate {
                source_id: id.clone(),
                coverage: against,
                scores: *scores,
            });
        }
    }

    if !complete.is_empty() {
        complete.sort_by(|left, right| {
            right
                .rank_key()
                .partial_cmp(&left.rank_key())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.source_id.cmp(&right.source_id))
        });
        return ReplacementOutcome::Found { ranked: complete };
    }

    partial.sort_by(|left, right| {
        right
            .coverage
            .completeness()
            .partial_cmp(&left.coverage.completeness())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.source_id.cmp(&right.source_id))
    });

    // What *no* candidate could supply: the intersection of the gaps, which is
    // the part of the coverage that is genuinely unavailable rather than
    // merely missing from one candidate.
    let uncovered = intersect_gaps(required, &partial);
    ReplacementOutcome::NotFound {
        considered: candidates.len(),
        uncovered,
        closest: partial.into_iter().take(3).collect(),
    }
}

fn intersect_gaps(required: &SourceCoverage, partial: &[PartialCandidate]) -> CoverageGap {
    let Some(first) = partial.first() else {
        // Nothing was even considered, so everything the lost source covered
        // is uncovered. Reporting an empty gap here would read as "nothing is
        // missing", which is the opposite of what an empty search means.
        return CoverageGap {
            asset_classes: required.asset_classes().clone(),
            regions: required.regions().clone(),
            instruments: required.instruments().clone(),
            frequency_shortfall: None,
        };
    };
    let mut uncovered = first.coverage.gap.clone();
    for candidate in partial.iter().skip(1) {
        let gap = &candidate.coverage.gap;
        uncovered.asset_classes = uncovered
            .asset_classes
            .intersection(&gap.asset_classes)
            .copied()
            .collect();
        uncovered.regions = uncovered
            .regions
            .intersection(&gap.regions)
            .copied()
            .collect();
        uncovered.instruments = uncovered
            .instruments
            .intersection(&gap.instruments)
            .cloned()
            .collect();
        if gap.frequency_shortfall.is_none() {
            uncovered.frequency_shortfall = None;
        }
    }
    uncovered
}
