//! The Meta Brain (R12-AMBIENT-3024, ADR 0466): the specialist whose subject
//! is the other specialists.
//!
//! Three questions, one per clause of the requirement:
//!
//! * **Which specialists are reliable in which regime.** Read from research
//!   memory, where the kernel writes every resolved analyst call with the
//!   regime it was made in (`ResearchMemory::reliability_by_regime`). One
//!   pooled hit rate describes an analyst right in a calm tape and wrong in a
//!   stressed one as mediocre in both, which is true of neither.
//! * **Where research attention should go.** The specialists measured in the
//!   regime in force, weighted by their record there. A specialist with no
//!   resolved call in that regime is *named* unmeasured and given no weight:
//!   an even-chance prior would invent a record the memory does not hold.
//! * **How a contested panel resolves.** Run after the panel, with its
//!   findings in hand (`Organisation::dispatch`'s second phase), the
//!   directional findings are re-weighed by each author's record in the
//!   regime. The resolution is a fact, not a vote: the finding is neutral
//!   at zero conviction, so the panel's own balance does not move and the
//!   meta brain is never a second, correlated copy of the panel it read.
//!
//! What it does not do: it does not change any specialist's conviction, does
//! not convene or drop anyone, and does not decide. REASON reads the
//! resolution beside the panel's balance; acting on either is REASON's
//! decision.

use crate::chief::{Arbiter, PanelView};
use crate::desk::Desk;
use crate::support::{FindingBuilder, computed};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::memory::RegimeRecord;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Reads the organisation's own track record and the panel it just heard.
#[derive(Debug)]
pub struct MetaBrain {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl MetaBrain {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

/// Each agent's record in the regime the brief names, or pooled over every
/// regime when it names none.
fn records_in(
    cells: &BTreeMap<(String, String), RegimeRecord>,
    regime: Option<&str>,
) -> BTreeMap<String, RegimeRecord> {
    let mut records: BTreeMap<String, RegimeRecord> = BTreeMap::new();
    for ((agent, cell_regime), record) in cells {
        if regime.is_some_and(|wanted| wanted != cell_regime) {
            continue;
        }
        let pooled = records.entry(agent.clone()).or_default();
        pooled.correct += record.correct;
        pooled.resolved += record.resolved;
    }
    records
}

/// The reliability-weighted balance of the panel's directional findings, in
/// `[-1, 1]`, and how many findings carried a weight. `None` where no
/// directional finding's author has a record, or every weight is zero: a
/// resolution nobody's record supports is not reported as one.
pub fn reliability_weighted_balance(
    panel: &[AgentFinding],
    records: &BTreeMap<String, RegimeRecord>,
) -> Option<(f64, usize)> {
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    let mut weighed = 0usize;
    for finding in panel {
        if finding.direction == Direction::Neutral {
            continue;
        }
        let Some(reliability) = records
            .get(&finding.agent_id)
            .and_then(RegimeRecord::hit_rate)
        else {
            continue;
        };
        let weight = finding.effective_conviction() * reliability;
        if !weight.is_finite() {
            continue;
        }
        numerator += finding.direction.sign() * weight;
        denominator += weight;
        weighed += 1;
    }
    (denominator > 1e-12).then(|| (numerator / denominator, weighed))
}

impl Agent for MetaBrain {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    /// Run outside the second phase it reads the empty panel: attention and
    /// reliability are still reported, and there is nothing to resolve.
    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        self.arbitrate(ctx, brief, &PanelView::default())
    }
}

impl Arbiter for MetaBrain {
    fn arbitrate(
        &self,
        ctx: &mut AgentContext,
        brief: &AgentBrief,
        panel: &PanelView,
    ) -> Result<AgentFinding> {
        let cells = {
            let memory = self.desk.memory.get(ctx)?;
            memory.reliability_by_regime(brief.as_of)
        };
        let regime = brief.regime.as_deref();
        let records = records_in(&cells, regime);
        let regimes: BTreeSet<&str> = cells.keys().map(|(_, r)| r.as_str()).collect();

        // Attention: the panel's authors when there is a panel, otherwise
        // every agent memory has a record for.
        let candidates: BTreeSet<String> = if panel.findings.is_empty() {
            records.keys().cloned().collect()
        } else {
            panel
                .findings
                .iter()
                .map(|f| f.agent_id.clone())
                .filter(|id| id != &self.manifest.id)
                .collect()
        };
        let measured: BTreeMap<&String, f64> = candidates
            .iter()
            .filter_map(|id| Some((id, records.get(id)?.hit_rate()?)))
            .collect();
        let unmeasured = candidates.len() - measured.len();
        let total: f64 = measured.values().sum();

        let regime_named = regime.unwrap_or("every regime, pooled");
        let mut claim = format!(
            "{} (agent, regime) record(s) across {} regime(s); in {regime_named}, {} specialist(s) \
             measured and {unmeasured} not",
            cells.len(),
            regimes.len(),
            measured.len()
        );
        let mut caveats = vec![
            "reliability is the share of resolved calls that were right; calibration of conviction \
             is the learning stage's"
                .to_string(),
            "the resolution is reported, never voted: this finding is neutral at zero conviction"
                .to_string(),
        ];
        if regime.is_none() {
            caveats.push(
                "the brief named no regime, so reliability is pooled over every regime".to_string(),
            );
        }

        // Every figure is gathered before the claim is written, so the claim
        // states exactly what the facts carry.
        let mut facts: Vec<(String, f64, &'static str, &'static [&'static str])> = vec![
            (
                "regimes_measured".into(),
                regimes.len() as f64,
                "count",
                &["research_memory"],
            ),
            (
                "attention_unmeasured".into(),
                unmeasured as f64,
                "count",
                &["research_memory"],
            ),
        ];

        // Reliability is evidenced only by a record: a memory holding no
        // resolved call with a regime has measured nobody, and a count of
        // zero under the scope's label would claim the scope regardless.
        if !cells.is_empty() {
            facts.push((
                "regime_reliability_cells".into(),
                cells.len() as f64,
                "count",
                &["research_memory"],
            ));
            for ((agent, cell_regime), record) in &cells {
                if let Some(rate) = record.hit_rate() {
                    facts.push((
                        format!("reliability:{agent}@{cell_regime}"),
                        rate,
                        "fraction",
                        &["research_memory"],
                    ));
                }
            }
        }

        // Weights only where some measured specialist has been right: if
        // every measured record is zero, proportional attention is undefined
        // and saying so beats spreading it evenly.
        if total > 1e-12 {
            facts.push((
                "attention_allocated".into(),
                measured.len() as f64,
                "count",
                &["research_memory"],
            ));
            for (agent, reliability) in &measured {
                facts.push((
                    format!("attention_weight:{agent}"),
                    reliability / total,
                    "fraction",
                    &["research_memory"],
                ));
            }
        } else if !measured.is_empty() {
            caveats.push(
                "every measured specialist has been wrong in this regime; no attention is allocated"
                    .to_string(),
            );
        }

        if panel.contested {
            match reliability_weighted_balance(&panel.findings, &records) {
                Some((balance, weighed)) => {
                    claim.push_str(&format!(
                        "; the contested panel resolves to {balance:+.3} on {weighed} finding(s) \
                         weighed by record"
                    ));
                    facts.push((
                        "reliability_weighted_balance".into(),
                        balance,
                        "balance",
                        &["panel_findings", "research_memory"],
                    ));
                    facts.push((
                        "disagreement_weighed_findings".into(),
                        weighed as f64,
                        "count",
                        &["panel_findings", "research_memory"],
                    ));
                }
                None => {
                    claim.push_str(
                        "; the panel is contested and no dissenting author has a record to weigh \
                         it by",
                    );
                    caveats.push(
                        "contested, unresolved: no directional finding's author has a resolved \
                         record in this regime"
                            .to_string(),
                    );
                }
            }
        }

        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(Direction::Neutral, 0.0)
            .evidence(vec![format!("research-memory@{}", brief.as_of)])
            .caveats(caveats);
        for (label, value, unit, sources) in facts {
            builder = builder.fact(computed(ctx, &label, value, unit, sources));
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(correct: usize, resolved: usize) -> RegimeRecord {
        RegimeRecord { correct, resolved }
    }

    #[test]
    fn records_in_a_named_regime_ignore_every_other_regime() {
        let mut cells = BTreeMap::new();
        cells.insert(("a".to_string(), "calm".to_string()), record(9, 10));
        cells.insert(("a".to_string(), "stressed".to_string()), record(1, 10));
        let calm = records_in(&cells, Some("calm"));
        assert_eq!(calm.get("a"), Some(&record(9, 10)));
        let pooled = records_in(&cells, None);
        assert_eq!(pooled.get("a"), Some(&record(10, 20)));
    }

    #[test]
    fn no_record_is_no_weight_rather_than_an_even_chance() {
        assert_eq!(record(0, 0).hit_rate(), None);
        let records = BTreeMap::new();
        assert_eq!(reliability_weighted_balance(&[], &records), None);
    }
}
