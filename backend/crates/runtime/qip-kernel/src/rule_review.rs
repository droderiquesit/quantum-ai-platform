//! Blueprint §12.3's per-rule accumulation: what the twin's scores say about
//! each risk rule, and the three findings the LEARN stage journals from it.
//!
//! The table in §12.3 has three rows about rules. A rule that vetoes mostly
//! *profitable* paths is too tight; one that vetoes mostly *losing* paths is
//! earning its place; one that almost never fires is dead weight. Until this
//! module the platform could key none of them, because a refusal was charged
//! to the control (`pre-trade-risk`) and never to the rule; commit `79f73f1`
//! carried the breach beside the sentence, and this is what reads it.
//!
//! Everything here is pure arithmetic over `&[DeclinedScore]` and a small
//! activity table — no I/O, no clock, `BTreeMap` throughout so a replay
//! iterates in the same order the cycle did. The consequences are split by
//! how much they may change:
//!
//! * A **defence** ([`RuleDefence`]) and a **dormancy finding**
//!   ([`RuleDormant`]) are records. They change nothing about the running
//!   set, and they are journaled so a rule's history says more than "fired".
//! * A **recalibration proposal** may only ever be a proposal. Nothing in this
//!   module, and nothing in the kernel, moves a bound in the process that
//!   computed it: §12.4's guardrail — "a veto rule may only be loosened
//!   through the full approval path, never automatically from counterfactual
//!   evidence" — is held by there being no code path that assigns a limit
//!   set after boot, which the acceptance suite scans for.
//!
//! The thresholds are the ones [`crate::platform::Platform::counterfactual_sizing_multiplier`]
//! already uses, by name, because the platform has one answer to "how many
//! observations make an evidence-weighted finding trustworthy" and a second,
//! differently-sized answer to the same question would be a number nobody
//! could reconcile with the first.

use crate::platform::{
    COUNTERFACTUAL_SIZING_MIN_SAMPLE, COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION, DeclinedScore,
};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::OrderId;
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_risk::limits::LimitKind;
use qip_twin::value::Simulated;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many cycles a rule may go without firing before it is recorded
/// dormant, provided [`RULE_DORMANCY_MIN_ORDERS`] were submitted meanwhile.
///
/// A hundred, and the two bars together rather than either alone: a rule
/// idle for a hundred cycles on a platform that submitted nothing is a rule
/// nothing has asked, not a rule that cannot fire, and a rule idle across a
/// hundred orders in three cycles has not been asked across enough market
/// states to say anything. The argument for the figure is in ADR 0061,
/// under "what would make this wrong": it is a stated bar, not a measured
/// one, and a desk whose cycle is a day will read it as a quarter.
pub(crate) const RULE_DORMANCY_CYCLES: u64 = 100;

/// How many orders must have been submitted while a rule stayed silent
/// before its silence is a finding. See [`RULE_DORMANCY_CYCLES`].
pub(crate) const RULE_DORMANCY_MIN_ORDERS: u64 = 100;

/// What one rule's scored refusals add up to.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleRegret {
    pub rule: String,
    /// Scored refusals charged to this rule. A path refused by two rules is
    /// in both samples.
    pub sample: usize,
    /// Of those, how many the twin says would have beaten standing aside.
    pub regrets: usize,
    /// What the regretted paths would have earned, summed. Simulated, and
    /// it stays that way.
    pub would_have_earned: Simulated<Decimal>,
    /// What the correctly declined paths would have lost: the magnitudes of
    /// the negative earnings over the `!regret` paths, summed. A correctly
    /// declined path that would have earned a little but not beaten standing
    /// aside contributes nothing here — it was not a loss avoided.
    pub would_have_lost: Simulated<Decimal>,
    /// Earliest refusal to latest scoring in the sample.
    pub window: (Timestamp, Timestamp),
    /// The most recently scored order in the sample: the idempotency key a
    /// finding built on this sample carries, so the same evidence reviewed
    /// on two cycles is one record.
    pub newest: OrderId,
    /// The observation, among the regretted paths' readings of this rule,
    /// farthest past the bound — the bound at which every regretted path
    /// would have been admitted. `None` where no regretted path carried a
    /// reading of this rule, which is every feasibility gate.
    pub admitting_bound: Option<f64>,
    /// Every scored order in the sample, in scoring order, so a proposal
    /// built on this can name the evidence it rests on.
    pub orders: Vec<OrderId>,
}

impl RuleRegret {
    /// The share of the sample the twin regrets. Zero on an empty sample.
    pub fn regret_fraction(&self) -> f64 {
        if self.sample == 0 {
            return 0.0;
        }
        // usize → f64: a ratio of counts, in the statistics lane.
        self.regrets as f64 / self.sample as f64
    }

    /// §12.3's first row: enough evidence, and most of it says the rule
    /// refused paths that would have paid.
    pub fn is_too_tight(&self) -> bool {
        self.sample >= COUNTERFACTUAL_SIZING_MIN_SAMPLE
            && self.regret_fraction() >= COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION
    }

    /// §12.3's second row: enough evidence, and most of it says the rule
    /// refused paths that would have lost.
    pub fn earns_its_place(&self) -> bool {
        self.sample >= COUNTERFACTUAL_SIZING_MIN_SAMPLE
            && (1.0 - self.regret_fraction()) >= COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION
    }
}

/// Accumulate the scored refusals by the rule each is charged to.
///
/// A score attributed to no rule — a posture refusal, or a risk refusal on
/// an unevaluated figure — contributes to nothing: there is no rule whose
/// bound it is evidence about. Keyed on the rule's configured name, in a
/// `BTreeMap`, so the journal writes findings in the same order every
/// replay.
pub fn regret_by_rule(scores: &[DeclinedScore]) -> BTreeMap<String, RuleRegret> {
    let mut by_rule: BTreeMap<String, RuleRegret> = BTreeMap::new();
    for score in scores {
        for rule in &score.rules {
            let entry = by_rule.entry(rule.clone()).or_insert_with(|| RuleRegret {
                rule: rule.clone(),
                sample: 0,
                regrets: 0,
                would_have_earned: Simulated::ZERO,
                would_have_lost: Simulated::ZERO,
                window: (score.declined_at, score.scored_at),
                newest: score.order_id.clone(),
                admitting_bound: None,
                orders: Vec::new(),
            });
            entry.sample += 1;
            entry.orders.push(score.order_id.clone());
            entry.window.0 = entry.window.0.min(score.declined_at);
            if score.scored_at >= entry.window.1 {
                entry.window.1 = score.scored_at;
                entry.newest = score.order_id.clone();
            }
            if score.regret {
                entry.regrets += 1;
                entry.would_have_earned = entry.would_have_earned + score.would_have_earned;
                for reading in score.readings.iter().filter(|r| &r.rule == rule) {
                    if !reading.observed.is_finite() || !reading.bound.is_finite() {
                        continue;
                    }
                    let distance = (reading.observed - reading.bound).abs();
                    let farther = match entry.admitting_bound {
                        None => true,
                        Some(current) => distance > (current - reading.bound).abs(),
                    };
                    if farther {
                        entry.admitting_bound = Some(reading.observed);
                    }
                }
            } else if score.would_have_earned.is_negative() {
                entry.would_have_lost = entry.would_have_lost + score.would_have_earned.abs();
            }
        }
    }
    by_rule
}

/// The evidence a recalibration proposal rests on, copied out of a
/// [`RuleRegret`] so the record is self-describing without the scores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegretEvidence {
    pub sample: usize,
    pub regrets: usize,
    pub regret_fraction: f64,
    /// What the regretted paths would have earned, summed. Simulated.
    pub would_have_earned: Simulated<Decimal>,
    pub window: (Timestamp, Timestamp),
    /// The newest scored order in the sample — the idempotency key.
    pub newest: OrderId,
    pub scored_orders: Vec<OrderId>,
}

impl RegretEvidence {
    pub fn of(regret: &RuleRegret) -> Self {
        Self {
            sample: regret.sample,
            regrets: regret.regrets,
            regret_fraction: regret.regret_fraction(),
            would_have_earned: regret.would_have_earned,
            window: regret.window,
            newest: regret.newest.clone(),
            scored_orders: regret.orders.clone(),
        }
    }
}

/// The outcome a [`RecalibrationProposal`] record carries.
pub const PROPOSAL_PROPOSED: &str = "proposed";
/// The evidence stopped clearing the bar before anybody signed.
pub const PROPOSAL_WITHDRAWN: &str = "withdrawn";
/// Two people signed and the artefact was emitted.
pub const PROPOSAL_ENACTED: &str = "enacted";

/// §12.3's first row: a proposal to loosen one rule's bound, generated from
/// regret evidence, and the only shape in which that row may exist.
///
/// A proposal is not a change. Nothing in the process that generated it
/// can install a bound — `Platform` has no setter for a limit set, the
/// acceptance suite scans for one — and what two signatures produce is an
/// artefact ([`qip_risk::limits::LimitSet::rebound`]) that a deployment
/// commits and mounts. §12.4's guardrail, "a veto rule may only be loosened
/// through the full approval path, never automatically from counterfactual
/// evidence", is held by that absence rather than by care at a call site.
///
/// [`Self::new`] is the only constructor, and it refuses a proposal that
/// does not loosen: a ceiling must rise and a floor must fall. There is no
/// route that supplies a bound — the API body carries a rationale and
/// nothing else — so a tightening cannot arrive as a proposal at all;
/// tightening is a reviewed commit to the limits file, which is the path a
/// desk already has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecalibrationProposal {
    pub rule: String,
    /// The limit's kind label, so the record says what sort of bound moved
    /// without a reader looking the rule up.
    pub kind: String,
    pub current_bound: f64,
    pub proposed_bound: f64,
    pub evidence: RegretEvidence,
    /// Generated from the evidence — never supplied by a caller.
    pub rationale: String,
    /// [`PROPOSAL_PROPOSED`], [`PROPOSAL_WITHDRAWN`] or [`PROPOSAL_ENACTED`].
    pub outcome: String,
    pub at: Timestamp,
}

impl EventBody for RecalibrationProposal {
    const TOPIC: Topic = Topic::RiskRuleRecalibration;
    const SCHEMA_VERSION: u32 = 1;

    /// One record per rule, per outcome, per body of evidence: the same
    /// sample reviewed on the next cycle is the same proposal.
    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}",
            self.rule,
            self.outcome,
            self.evidence.newest.as_str()
        ))
    }
}

impl RecalibrationProposal {
    /// Build a proposal, or refuse one that is not a loosening on enough
    /// evidence.
    pub fn new(
        rule: &str,
        kind: &LimitKind,
        proposed_bound: f64,
        evidence: RegretEvidence,
        at: Timestamp,
    ) -> Result<Self> {
        if evidence.sample < COUNTERFACTUAL_SIZING_MIN_SAMPLE {
            return Err(Error::invalid(format!(
                "a recalibration of {rule} needs at least {COUNTERFACTUAL_SIZING_MIN_SAMPLE} \
                 scored refusals and has {}; below that a pattern is as likely noise as a finding",
                evidence.sample
            )));
        }
        let current_bound = kind.bound();
        if !proposed_bound.is_finite() || !current_bound.is_finite() {
            return Err(Error::numeric(format!(
                "a recalibration of {rule} from {current_bound} to {proposed_bound} is not a \
                 comparison between two finite numbers"
            )));
        }
        let loosens = if kind.is_minimum() {
            proposed_bound < current_bound
        } else {
            proposed_bound > current_bound
        };
        if !loosens {
            return Err(Error::invalid(format!(
                "a recalibration of {rule} from {current_bound} to {proposed_bound} would not \
                 loosen a {}; regret evidence can only ever argue that a rule refused too much, \
                 and a tightening is a reviewed change to the limits file rather than a proposal",
                if kind.is_minimum() {
                    "floor"
                } else {
                    "ceiling"
                }
            )));
        }
        let rationale = format!(
            "{} of {} paths refused by {rule} between {} and {} would have beaten standing \
             aside, earning {} in simulation; a bound of {proposed_bound} instead of \
             {current_bound} would have admitted every one of them",
            evidence.regrets,
            evidence.sample,
            evidence.window.0.to_rfc3339(),
            evidence.window.1.to_rfc3339(),
            evidence.would_have_earned
        );
        Ok(Self {
            rule: rule.to_string(),
            kind: kind.label().to_string(),
            current_bound,
            proposed_bound,
            evidence,
            rationale,
            outcome: PROPOSAL_PROPOSED.to_string(),
            at,
        })
    }

    /// The same proposal with its outcome moved and its instant restated.
    pub fn with_outcome(&self, outcome: &str, at: Timestamp) -> Self {
        Self {
            outcome: outcome.to_string(),
            at,
            ..self.clone()
        }
    }

    pub fn is_open(&self) -> bool {
        self.outcome == PROPOSAL_PROPOSED
    }
}

/// §12.3's second row on the record: a rule whose declined paths were mostly
/// correctly declined, with the simulated loss it avoided.
///
/// Changes nothing. It exists so that a rule's history in the log can say
/// "this refused ten paths and eight of them would have lost money" rather
/// than only that it fired — and so that the mirror finding, a proposal to
/// loosen, can be read beside the evidence that a neighbouring rule is
/// doing exactly what it is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleDefence {
    pub rule: String,
    pub sample: usize,
    pub correctly_declined: usize,
    /// The magnitudes of the negative simulated earnings over the correctly
    /// declined paths, summed. Simulated, and no arithmetic here lets it out.
    pub would_have_lost: Simulated<Decimal>,
    pub window: (Timestamp, Timestamp),
    /// The newest scored order the defence rests on — the idempotency key.
    pub newest: OrderId,
    pub at: Timestamp,
}

impl EventBody for RuleDefence {
    const TOPIC: Topic = Topic::RiskRuleDefended;
    const SCHEMA_VERSION: u32 = 1;

    /// One defence per rule per body of evidence: the same sample reviewed
    /// on the next cycle is the same finding, and the log holds it once.
    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.rule, self.newest.as_str()))
    }
}

/// §12.3's third row on the record: a rule that has not fired for
/// [`RULE_DORMANCY_CYCLES`] cycles while [`RULE_DORMANCY_MIN_ORDERS`] orders
/// were submitted.
///
/// A finding and not a removal. This repository's standing example of what
/// not to ship is a limit that could never fire and read as protection;
/// dormancy is the observable half of that defect, and the record is what
/// lets a person ask whether the rule can still bind or has quietly become
/// the next `MaxExpectedShortfall`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleDormant {
    pub rule: String,
    pub idle_cycles: u64,
    pub orders_submitted_meanwhile: u64,
    /// The cycle the rule last fired on, or zero for a rule that has never
    /// fired since assembly. The idempotency key, so one episode of silence
    /// is one record however many cycles it lasts, and a rule that fires
    /// and falls silent again is a second episode and a second record.
    pub since: u64,
    pub at: Timestamp,
}

impl EventBody for RuleDormant {
    const TOPIC: Topic = Topic::RiskRuleDormant;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.rule, self.since))
    }
}

/// One rule's firing history, as the platform keeps it between cycles.
///
/// Every limit name in the boot set has a row from assembly, so a rule that
/// never fires still has somewhere for its silence to be measured; a table
/// that gained rows on the first fire would be silent about exactly the
/// rules the dormancy finding is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuleActivity {
    pub fires: u64,
    pub last_fired_cycle: Option<u64>,
    /// `Platform::orders_submitted` as it stood at the last fire.
    pub submitted_at_last_fire: u64,
    /// `Some(since)` while the rule stands recorded dormant; cleared by the
    /// next fire, so a second episode is measured from that fire.
    pub dormant_since: Option<u64>,
}

/// What one newly dormant rule measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dormancy {
    pub idle_cycles: u64,
    pub idle_orders: u64,
    pub since: u64,
}

impl RuleActivity {
    pub(crate) const fn new() -> Self {
        Self {
            fires: 0,
            last_fired_cycle: None,
            submitted_at_last_fire: 0,
            dormant_since: None,
        }
    }

    /// The rule refused an order on `cycle`, with `orders_submitted` orders
    /// accepted so far. Clears a standing dormancy: the finding was that the
    /// rule was not binding, and it just did.
    pub(crate) fn fired(&mut self, cycle: u64, orders_submitted: u64) {
        self.fires += 1;
        self.last_fired_cycle = Some(cycle);
        self.submitted_at_last_fire = orders_submitted;
        self.dormant_since = None;
    }

    /// Whether the rule has just crossed both dormancy bars and is not
    /// already recorded dormant for this episode.
    pub(crate) fn dormancy(&self, cycle: u64, orders_submitted: u64) -> Option<Dormancy> {
        let since = self.last_fired_cycle.unwrap_or(0);
        let idle_cycles = cycle.saturating_sub(since);
        let idle_orders = orders_submitted.saturating_sub(self.submitted_at_last_fire);
        if idle_cycles >= RULE_DORMANCY_CYCLES
            && idle_orders >= RULE_DORMANCY_MIN_ORDERS
            && self.dormant_since.is_none()
        {
            Some(Dormancy {
                idle_cycles,
                idle_orders,
                since,
            })
        } else {
            None
        }
    }
}

/// What the LEARN stage's rule review left in the cycle's journal entry.
///
/// Rule names only, in the order the findings were written. Absent from the
/// entry on a cycle that found nothing, so a cycle on a platform whose rules
/// all fire and none regrets reads exactly as it did before this existed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleReviewJournal {
    /// Rules defended this cycle — a new defence record written.
    pub defended: Vec<String>,
    /// Rules newly recorded dormant this cycle.
    pub dormant: Vec<String>,
    /// Rules a recalibration was proposed for this cycle. Defaulted so an
    /// entry journaled before the field existed replays.
    #[serde(default)]
    pub proposed: Vec<String>,
    /// Rules whose open proposal was withdrawn this cycle because the
    /// evidence stopped clearing the bar.
    #[serde(default)]
    pub withdrawn: Vec<String>,
}

impl RuleReviewJournal {
    pub fn is_empty(&self) -> bool {
        self.defended.is_empty()
            && self.dormant.is_empty()
            && self.proposed.is_empty()
            && self.withdrawn.is_empty()
    }

    /// One line for the stage summary.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.proposed.is_empty() {
            parts.push(format!(
                "{} recalibration(s) proposed ({})",
                self.proposed.len(),
                self.proposed.join(", ")
            ));
        }
        if !self.withdrawn.is_empty() {
            parts.push(format!(
                "{} proposal(s) withdrawn ({})",
                self.withdrawn.len(),
                self.withdrawn.join(", ")
            ));
        }
        if !self.defended.is_empty() {
            parts.push(format!(
                "{} rule(s) defended ({})",
                self.defended.len(),
                self.defended.join(", ")
            ));
        }
        if !self.dormant.is_empty() {
            parts.push(format!(
                "{} rule(s) recorded dormant ({})",
                self.dormant.len(),
                self.dormant.join(", ")
            ));
        }
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn evidence(sample: usize) -> RegretEvidence {
        let at = Timestamp::from_secs(1_760_000_000);
        RegretEvidence {
            sample,
            regrets: sample,
            regret_fraction: 1.0,
            would_have_earned: Simulated::of(dec!("1000")),
            window: (at, at),
            newest: OrderId::from_string("ord-newest"),
            scored_orders: Vec::new(),
        }
    }

    #[test]
    fn a_proposal_cannot_tighten_a_bound() {
        // The property the whole row rests on: regret evidence can only ever
        // argue that a rule refused too much, so the one constructor refuses
        // a bound that would not loosen — a ceiling that does not rise, a
        // floor that does not fall, and either left where it is. Tightening
        // is a reviewed commit to the limits file, not a proposal.
        let at = Timestamp::from_secs(1_760_000_000);
        let ceiling = LimitKind::MaxOrderNotional {
            limit: Decimal::from_int(250_000),
        };
        let floor = LimitKind::MinCashBuffer { limit: 0.02 };

        // The admitting half first, so the refusals below are choices and
        // not a constructor that refuses everything.
        let loosened =
            RecalibrationProposal::new("order-notional", &ceiling, 300_000.0, evidence(12), at)
                .expect("a ceiling raised on enough evidence is a proposal");
        assert_eq!(loosened.outcome, PROPOSAL_PROPOSED);
        assert!((loosened.current_bound - 250_000.0).abs() < 1e-9);
        assert!((loosened.proposed_bound - 300_000.0).abs() < 1e-9);
        RecalibrationProposal::new("cash-buffer", &floor, 0.01, evidence(12), at)
            .expect("a floor lowered on enough evidence is a proposal");

        for (rule, kind, bound) in [
            ("order-notional", &ceiling, 200_000.0),
            ("order-notional", &ceiling, 250_000.0),
            ("cash-buffer", &floor, 0.03),
            ("cash-buffer", &floor, 0.02),
        ] {
            let refused = RecalibrationProposal::new(rule, kind, bound, evidence(12), at)
                .expect_err("a bound that does not loosen became a proposal");
            assert!(
                refused.message().contains("would not loosen"),
                "the refusal does not say why: {}",
                refused.message()
            );
        }

        // And not on thin evidence, whichever way it points.
        let thin =
            RecalibrationProposal::new("order-notional", &ceiling, 300_000.0, evidence(9), at)
                .expect_err("nine scored refusals became a proposal");
        assert!(refused_for_sample(&thin), "{}", thin.message());
        // Nor on a bound that is not a number.
        RecalibrationProposal::new("order-notional", &ceiling, f64::NAN, evidence(12), at)
            .expect_err("a NaN bound became a proposal");
    }

    fn refused_for_sample(error: &Error) -> bool {
        error.message().contains("needs at least")
    }
}
