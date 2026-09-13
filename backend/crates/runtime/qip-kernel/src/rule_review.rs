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
use qip_core::ids::OrderId;
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
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
            });
            entry.sample += 1;
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
}

impl RuleReviewJournal {
    pub fn is_empty(&self) -> bool {
        self.defended.is_empty() && self.dormant.is_empty()
    }

    /// One line for the stage summary.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
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
