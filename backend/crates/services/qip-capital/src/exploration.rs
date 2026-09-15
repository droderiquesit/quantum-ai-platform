//! The exploration budget: capital allocated to information gain rather than
//! expected return (blueprint §13.2).
//!
//! The failure this module removes is a budget that exists only as a number
//! in a mandate. [`Mandate::exploration_share`](super::ledger::Mandate::exploration_share)
//! has been validated, stored and rendered in the console since the user
//! ledger shipped, and nothing drew on it: there was no probe, no rule for
//! choosing one, no bound on what one could lose and no account in which the
//! cost of learning was visible apart from the cost of trading. A share
//! nothing spends is not a budget; it is a field.
//!
//! Five things, and each is one row of §13.2's allocation table.
//!
//! * **Budget size** — [`budget_for`], a stated share of deployable capital.
//!   The share comes from a mandate and is never inferred, never defaulted
//!   upward and never clamped: a share above one is a caller who confused
//!   percent with fraction and is refused, because a budget quietly corrected
//!   to the whole book explores with everything while the caller believes it
//!   is exploring with a fifth.
//! * **Probe types** — [`ProbeKind`], the five in the table. Each names what
//!   it learns, so a reader of a plan can see what was being bought.
//! * **Selection** — [`ExplorationBook::plan`], an upper-confidence-bound rule
//!   over the uncertainty each candidate carries, weighted by the value of
//!   resolving it. UCB and not Thompson sampling, deliberately: Thompson
//!   needs a draw from a posterior, and nothing in this crate draws a random
//!   number or reads a clock — every entry point takes the
//!   [`Timestamp`] it reasons about, so a replay reproduces the same probes
//!   in the same order. A sampler would make the exploration budget the one
//!   capital decision a replay could not reproduce.
//! * **Bounds** — every probe carries a maximum loss, no probe may risk more
//!   than [`MAXIMUM_PROBE_SHARE`] of the budget, and the sum of the live
//!   probes can never exceed the budget. A candidate that asks for more is
//!   *declined and named* ([`ExplorationPlan::declined`]) rather than shrunk
//!   to fit: a probe resized behind the caller's back is a probe whose stated
//!   bound is no longer the bound it ran under.
//! * **Accounting and value measurement** — [`ExplorationBook`] keeps what
//!   exploration committed and what it actually spent apart from
//!   return-seeking capital, and scores each probe by the uncertainty it
//!   resolved, so the budget is optimised from measurement rather than
//!   from belief.
//!
//! # An exploration trade never becomes a position
//!
//! Structurally, not by convention. A [`Probe`] carries a subject, a bound
//! and an expiry and has no instrument, no side and no quantity; there is no
//! constructor, conversion or method anywhere that turns one into an order,
//! and a probe that is never taken up leaves through
//! [`ExplorationBook::settle`] or [`ExplorationBook::abandon`], both of which
//! return its capital to the budget. The type cannot name what it would
//! trade, so nothing downstream can trade it.
//!
//! # The two kinds of evidence, kept apart
//!
//! [`ProbeEvidence`] distinguishes a subject whose uncertainty moved *because
//! a probe was taken up* from one that moved while the probe sat unexercised.
//! Only the first feeds the selection rule's measured reward. This matters
//! today rather than in principle: no execution path takes a probe up, so
//! everything the platform currently settles is `Observed`, and a book that
//! folded those into the reward would report that probing works on evidence
//! that no probing happened.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The largest share of the budget any single probe may put at risk.
///
/// A quarter, so a budget always funds at least four questions. One probe
/// that could risk the whole budget is not an exploration programme; it is a
/// position with a research label on it.
pub const MAXIMUM_PROBE_SHARE: Decimal = Decimal::from_raw(250_000_000);

/// Probes that may be open at once. A bound on the working set, like every
/// other working set in this platform, and the reason a plan can decline a
/// candidate it has budget for.
pub const MAXIMUM_OPEN_PROBES: usize = 8;

/// Subjects whose probe history the book remembers.
///
/// Bounded because the subject is a caller's string: a kernel that renamed a
/// component every cycle would otherwise grow this map for the life of the
/// process. Eviction is recorded ([`ExplorationBook::subjects_forgotten`]),
/// never silent — a forgotten subject looks unprobed to the selection rule,
/// and that is a fact about the rule a reader has to be able to see.
pub const MAXIMUM_TRACKED_SUBJECTS: usize = 512;

/// Pseudo-observations the candidate's own uncertainty counts for when a
/// kind has measured outcomes to blend with it.
///
/// The same shape, and the same four, as the self model's sample-size
/// shrinkage: one settled probe does not overturn the prior, and a hundred
/// nearly replace it.
pub const PRIOR_WEIGHT: f64 = 4.0;

/// The `c` in the upper-confidence bound.
///
/// UCB1's own constant, for rewards expressed in `[0, 1]` — which is what
/// [`ExplorationBook::plan`] scores on, the fraction of a subject's
/// uncertainty a probe is expected to resolve. Raising it probes more
/// rarely-touched subjects and fewer promising ones; it is stated here so
/// that trade-off is a diff somebody reviews rather than a tuning constant
/// inside a loop.
pub const EXPLORATION_CONSTANT: f64 = std::f64::consts::SQRT_2;

/// What a probe buys, and at what cost — §13.2's table, as types.
///
/// The kinds are the axis a measured reward is kept on, so they are also the
/// label set any metric over probes may use: five values fixed in source,
/// never a subject, never an instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProbeKind {
    /// Trade a strategy at a multiple of normal size, once, to find where its
    /// capacity actually decays rather than where the model says it does.
    CapacityAtSize,
    /// Post a resting order on an unfamiliar venue, to learn its fill and
    /// adverse-selection behaviour.
    UnfamiliarVenue,
    /// Take a small position where the model is uncertain, to learn whether
    /// the uncertainty is irreducible or merely unobserved.
    UncertainModel,
    /// Trade a rarely-active strategy deliberately, to refresh a stale model
    /// and a stale capacity estimate.
    StaleEstimate,
    /// Enter a regime-boundary trade, to learn how the causal edges behave at
    /// the transition.
    RegimeBoundary,
}

impl ProbeKind {
    /// Every kind, in declaration order — the order a report lists them in.
    pub const ALL: [Self; 5] = [
        Self::CapacityAtSize,
        Self::UnfamiliarVenue,
        Self::UncertainModel,
        Self::StaleEstimate,
        Self::RegimeBoundary,
    ];

    /// The stable token a label or a journal entry carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CapacityAtSize => "capacity_at_size",
            Self::UnfamiliarVenue => "unfamiliar_venue",
            Self::UncertainModel => "uncertain_model",
            Self::StaleEstimate => "stale_estimate",
            Self::RegimeBoundary => "regime_boundary",
        }
    }

    /// What a probe of this kind is bought for, in the blueprint's words. On
    /// the type rather than at the call site so a plan can say what it was
    /// buying without every caller restating the table.
    pub fn learns(self) -> &'static str {
        match self {
            Self::CapacityAtSize => "where a strategy's capacity actually decays",
            Self::UnfamiliarVenue => "a venue's fill and adverse-selection behaviour",
            Self::UncertainModel => "whether an uncertainty is irreducible or merely unobserved",
            Self::StaleEstimate => "whether a stale model and capacity estimate still hold",
            Self::RegimeBoundary => "how the causal edges behave at a regime transition",
        }
    }
}

/// A question the platform could buy an answer to, and what it would cost.
///
/// Validated at construction and nothing corrected. An uncertainty outside
/// `(0, 1]` is a caller who has not measured what they think they measured,
/// and a probe sized from it would be a number about nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeCandidate {
    pub kind: ProbeKind,
    /// What is uncertain: a component, a venue, a strategy. The key the
    /// book's probe history is kept on, so it must be the same string for
    /// the same question on every pass.
    pub subject: String,
    /// How much of this subject is unresolved, in `(0, 1]`. One means
    /// nothing is known; a subject at zero has nothing to learn and is
    /// refused rather than ranked last, because a candidate that can never
    /// be selected is a row in a plan that reads as a choice.
    pub uncertainty: f64,
    /// What resolving it is worth, in the mandate's currency. Money, so
    /// [`Decimal`].
    pub resolution_value: Decimal,
    /// The most this probe may lose. Positive, and the bound the plan
    /// enforces against the budget.
    pub maximum_loss: Decimal,
}

impl ProbeCandidate {
    /// Refuse by field name, naming what to do instead.
    pub fn new(
        kind: ProbeKind,
        subject: impl Into<String>,
        uncertainty: f64,
        resolution_value: Decimal,
        maximum_loss: Decimal,
    ) -> Result<Self> {
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(Error::invalid(
                "a probe candidate needs a subject; an unnamed question cannot be scored, \
                 cannot be counted against a probe history and cannot be reported",
            ));
        }
        if !uncertainty.is_finite() || uncertainty <= 0.0 || uncertainty > 1.0 {
            return Err(Error::invalid(format!(
                "uncertainty {uncertainty} for {subject} is not a fraction in (0, 1]; a \
                 subject with nothing unresolved is not a candidate, and a value above one \
                 is a caller who has not normalised their measure"
            )));
        }
        if resolution_value.is_negative() {
            return Err(Error::invalid(format!(
                "the value of resolving {subject} cannot be negative ({resolution_value}); a \
                 question worth less than nothing is one not to ask, not one to pay for"
            )));
        }
        if !maximum_loss.is_positive() {
            return Err(Error::invalid(format!(
                "the maximum loss for {subject} must be positive, not {maximum_loss}; a probe \
                 with no bound is a position, and one bounded at zero buys nothing"
            )));
        }
        Ok(Self {
            kind,
            subject,
            uncertainty,
            resolution_value,
            maximum_loss,
        })
    }

    /// The value of resolving this question per unit of capital risked.
    ///
    /// Money over money, so the ratio is dimensionless and comparable across
    /// candidates — the crossing point from [`Decimal`] to `f64` is here,
    /// deliberately and in one place: everything above is money and
    /// everything below is a score.
    fn value_weight(&self) -> f64 {
        let risked = self.maximum_loss.to_f64();
        if risked <= 0.0 {
            // Unreachable: the constructor refuses a non-positive bound.
            // Scored at zero rather than dividing, because a division here
            // would put an infinity into an ordering.
            return 0.0;
        }
        self.resolution_value.to_f64() / risked
    }
}

/// A probe the plan selected: what is being asked, what it may lose, and
/// when the question closes.
///
/// No instrument, no side, no quantity — see the module documentation. What
/// a probe *would* trade is the caller's business and never travels in this
/// type, which is what stops an exploration allocation from becoming a
/// position through an ordinary conversion somebody adds later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub id: String,
    pub kind: ProbeKind,
    pub subject: String,
    pub maximum_loss: Decimal,
    /// The uncertainty the subject carried when the probe opened — the
    /// baseline the information gain is measured against. Recorded at open
    /// rather than re-derived at settlement, because a baseline taken after
    /// the fact is a baseline chosen to suit the answer.
    pub uncertainty_at_open: f64,
    pub opened_at: Timestamp,
    pub expires_at: Timestamp,
    /// The score that selected it, kept so a reader can see why this question
    /// was bought and another was not.
    pub score: f64,
}

impl Probe {
    pub fn is_expired(&self, now: Timestamp) -> bool {
        now >= self.expires_at
    }
}

/// A candidate the plan did not take, with the reason stated.
///
/// Every decline is reported. A selection rule that silently dropped the
/// candidates it could not afford would look identical to one with nothing
/// to consider, and those are opposite states of the same budget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeclinedProbe {
    pub kind: ProbeKind,
    pub subject: String,
    pub reason: String,
}

/// What one pass of the selection rule decided.
///
/// Always produced, never `Option`. A plan over no candidates, or against no
/// budget, is a plan that says so — [`Self::summary`] is never empty — because
/// the state a deployment is usually in is the idle one, and a module that
/// returns nothing when it has nothing to say reaches no surface at all in
/// exactly that state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplorationPlan {
    /// The whole budget this plan was drawn against.
    pub budget: Decimal,
    /// Already held by probes open before this pass.
    pub committed_before: Decimal,
    /// Newly committed by the probes this plan selected.
    pub committed_now: Decimal,
    pub selected: Vec<Probe>,
    pub declined: Vec<DeclinedProbe>,
    /// How many candidates were scored. The premise of every count above it:
    /// nothing selected out of nothing considered is a different cycle from
    /// nothing selected out of forty.
    pub considered: usize,
}

impl ExplorationPlan {
    /// Budget neither held by a live probe nor committed by this plan.
    pub fn uncommitted(&self) -> Decimal {
        self.budget - self.committed_before - self.committed_now
    }

    /// One line an operator can read, in every state including the idle one.
    ///
    /// The idle states are spelled out rather than left as a zero: "no share
    /// is set aside" and "nothing is uncertain enough to ask about" are
    /// different facts about a platform, and a report that rendered both as
    /// `0 probes` would hide which one holds.
    pub fn summary(&self) -> String {
        if !self.budget.is_positive() {
            return format!(
                "exploration: no budget — no mandate sets a share aside, so none of the \
                 {} candidate(s) can be probed",
                self.considered
            );
        }
        if self.selected.is_empty() {
            return format!(
                "exploration: {} budget, {} already committed, {} candidate(s) considered and \
                 none selected{}",
                self.budget,
                self.committed_before,
                self.considered,
                declined_tail(&self.declined),
            );
        }
        let kinds: Vec<String> = self
            .selected
            .iter()
            .map(|probe| format!("{} on {}", probe.kind.as_str(), probe.subject))
            .collect();
        format!(
            "exploration: {} budget, {} newly committed to {} probe(s) of {} considered ({}){}",
            self.budget,
            self.committed_now,
            self.selected.len(),
            self.considered,
            kinds.join(", "),
            declined_tail(&self.declined),
        )
    }
}

/// The declines, as a clause a summary can append. Named rather than counted:
/// "two declined" tells an operator something was refused without saying what
/// they could do about it.
fn declined_tail(declined: &[DeclinedProbe]) -> String {
    if declined.is_empty() {
        return String::new();
    }
    let named: Vec<String> = declined
        .iter()
        .map(|probe| format!("{} ({})", probe.subject, probe.reason))
        .collect();
    format!("; declined: {}", named.join(", "))
}

/// Whether a subject's uncertainty moved because a probe was taken up, or
/// while it sat unexercised.
///
/// Kept apart because only the first is evidence about probing. See the
/// module documentation: today every settlement the platform makes is
/// `Observed`, and folding those into the measured reward would report that
/// exploration works on evidence that no exploration happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProbeEvidence {
    /// The probe was taken up: capital was risked and the answer is the
    /// probe's.
    Probed,
    /// The probe expired unexercised and the subject was re-measured anyway.
    /// A fact about the subject, not about the probe.
    Observed,
}

impl ProbeEvidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Probed => "probed",
            Self::Observed => "observed",
        }
    }
}

/// What became of a probe: what it cost, and what it resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeOutcome {
    pub evidence: ProbeEvidence,
    /// What was actually spent. Zero for a probe nothing took up — and zero
    /// is recorded as zero rather than as the bound, because billing what was
    /// planned instead of what ran is how an exploration cost becomes a
    /// number nobody can check.
    pub realised_cost: Decimal,
    /// The subject's uncertainty now, on the same measure the candidate
    /// carried. In `[0, 1]`.
    pub uncertainty_now: f64,
}

impl ProbeOutcome {
    pub fn new(
        evidence: ProbeEvidence,
        realised_cost: Decimal,
        uncertainty_now: f64,
    ) -> Result<Self> {
        if realised_cost.is_negative() {
            return Err(Error::invalid(format!(
                "a probe's realised cost cannot be negative ({realised_cost}); an exploration \
                 that made money is still an exploration that cost zero — record the profit in \
                 the book it was earned in, not as a negative cost here"
            )));
        }
        if !uncertainty_now.is_finite() || !(0.0..=1.0).contains(&uncertainty_now) {
            return Err(Error::invalid(format!(
                "the uncertainty after a probe ({uncertainty_now}) is not a fraction in [0, 1]; \
                 an information gain computed from it would be a number about nothing"
            )));
        }
        Ok(Self {
            evidence,
            realised_cost,
            uncertainty_now,
        })
    }
}

/// What one settled probe is recorded as.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeSettlement {
    pub id: String,
    pub kind: ProbeKind,
    pub subject: String,
    pub evidence: ProbeEvidence,
    pub realised_cost: Decimal,
    /// Uncertainty at open less uncertainty now: positive where the question
    /// got clearer, negative where it got murkier. A statistic, so `f64`.
    pub information_gain: f64,
    /// Whether the cost stayed inside the probe's own bound. A breach is
    /// recorded rather than refused: the money is already spent, and a
    /// settlement the book rejected would leave the capital committed forever
    /// and the overspend unrecorded.
    pub within_bound: bool,
}

/// What the book has measured about one kind of probe.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KindRecord {
    pub opened: u64,
    /// Settlements on probed evidence — the only ones that feed the reward.
    pub probed: u64,
    /// Settlements on observed evidence, counted and reported, never scored.
    pub observed: u64,
    /// The sum of the information gains on probed evidence.
    pub probed_gain: f64,
    /// Everything this kind has actually spent, probed or not.
    pub realised_cost: Decimal,
    /// Settlements whose cost exceeded the probe's own bound.
    pub bound_breaches: u64,
}

impl KindRecord {
    /// The mean information gain per probed settlement, or `None` where this
    /// kind has never been taken up. `None` means "not measured", which is a
    /// different statement from a measured zero and must stay tellable from
    /// it.
    pub fn measured_gain(&self) -> Option<f64> {
        if self.probed == 0 {
            return None;
        }
        // usize/u64 → f64: a mean of statistics, not money.
        Some(self.probed_gain / self.probed as f64)
    }
}

/// The exploration account: what is open, what it has cost, and what it has
/// resolved.
///
/// Serializable so a deployment can journal and replay it, and every map a
/// [`BTreeMap`] so a report built from it comes out in the same order on
/// every machine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExplorationBook {
    open: BTreeMap<String, Probe>,
    kinds: BTreeMap<ProbeKind, KindRecord>,
    attempts: BTreeMap<String, u32>,
    opened_total: u64,
    settled_total: u64,
    abandoned_total: u64,
    /// Everything exploration has actually spent, across every kind. The
    /// number that must be reported apart from return-seeking performance.
    spend: Decimal,
    sequence: u64,
    subjects_forgotten: u64,
}

impl ExplorationBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// The live probes, in id order.
    pub fn open(&self) -> impl Iterator<Item = &Probe> {
        self.open.values()
    }

    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// What the live probes have between them put at risk.
    pub fn committed(&self) -> Decimal {
        self.open
            .values()
            .map(|probe| probe.maximum_loss)
            .fold(Decimal::ZERO, |sum, loss| sum + loss)
    }

    /// What exploration has spent. Never mixed with return-seeking capital:
    /// this is the figure §13.2 requires be reported on its own, so that the
    /// cost of learning reads as the cost of learning rather than as drag.
    pub fn spend(&self) -> Decimal {
        self.spend
    }

    pub fn opened_total(&self) -> u64 {
        self.opened_total
    }

    pub fn settled_total(&self) -> u64 {
        self.settled_total
    }

    pub fn abandoned_total(&self) -> u64 {
        self.abandoned_total
    }

    pub fn subjects_forgotten(&self) -> u64 {
        self.subjects_forgotten
    }

    pub fn record(&self, kind: ProbeKind) -> Option<&KindRecord> {
        self.kinds.get(&kind)
    }

    /// How many probes this subject has already had.
    pub fn attempts(&self, subject: &str) -> u32 {
        self.attempts.get(subject).copied().unwrap_or(0)
    }

    /// The probes whose question has closed, in id order.
    ///
    /// Returned rather than swept, because closing one is a decision with an
    /// outcome attached — [`Self::settle`] with what the subject now measures,
    /// or [`Self::abandon`] where it can no longer be measured at all.
    pub fn due(&self, now: Timestamp) -> Vec<Probe> {
        self.open
            .values()
            .filter(|probe| probe.is_expired(now))
            .cloned()
            .collect()
    }

    /// Choose what to probe, against a budget, without changing anything.
    ///
    /// The rule, stated once:
    ///
    /// ```text
    /// expected_gain = (PRIOR_WEIGHT * uncertainty + Σ probed gains for the kind)
    ///                 / (PRIOR_WEIGHT + probed settlements for the kind)
    /// bonus         = EXPLORATION_CONSTANT * sqrt(ln(1 + probes opened) / (1 + attempts))
    /// score         = (resolution_value / maximum_loss) * (expected_gain + bonus)
    /// ```
    ///
    /// The bracket is UCB1 over rewards in `[0, 1]` — the fraction of a
    /// subject's uncertainty a probe of that kind is expected to resolve,
    /// shrunk toward the candidate's own prior by [`PRIOR_WEIGHT`]
    /// pseudo-observations. The factor in front is §13.2's "weighted by the
    /// value of resolving it": two equally uncertain questions are not
    /// equally worth asking if one of them governs a hundred times the
    /// capital. Multiplying rather than adding keeps the whole score in one
    /// unit — value per unit of capital risked — so a candidate cannot buy
    /// rank by being cheap in a term the other half of the sum does not see.
    ///
    /// Ties break on kind then subject, so two replays of the same evidence
    /// select the same probes in the same order.
    pub fn plan(
        &self,
        budget: Decimal,
        candidates: &[ProbeCandidate],
        validity: Duration,
        now: Timestamp,
    ) -> Result<ExplorationPlan> {
        if budget.is_negative() {
            return Err(Error::invalid(format!(
                "an exploration budget cannot be negative ({budget}); reconcile the capital it \
                 is a share of before planning against it"
            )));
        }
        if validity.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a probe needs a positive validity; one that expires at or before it opens \
                 holds capital and asks nothing",
            ));
        }
        let committed_before = self.committed();
        let mut plan = ExplorationPlan {
            budget,
            committed_before,
            committed_now: Decimal::ZERO,
            selected: Vec::new(),
            declined: Vec::new(),
            considered: candidates.len(),
        };
        if !budget.is_positive() {
            for candidate in candidates {
                plan.declined.push(DeclinedProbe {
                    kind: candidate.kind,
                    subject: candidate.subject.clone(),
                    reason: "no exploration budget is set aside".to_string(),
                });
            }
            return Ok(plan);
        }

        let ceiling = budget.checked_mul(MAXIMUM_PROBE_SHARE).ok_or_else(|| {
            Error::numeric(format!(
                "the per-probe ceiling on a budget of {budget} overflows"
            ))
        })?;

        let mut scored: Vec<(f64, &ProbeCandidate)> = candidates
            .iter()
            .map(|candidate| (self.score(candidate), candidate))
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.kind.cmp(&right.1.kind))
                .then_with(|| left.1.subject.cmp(&right.1.subject))
        });

        let mut remaining = budget - committed_before;
        let mut sequence = self.sequence;
        for (score, candidate) in scored {
            let reason = if self
                .open
                .values()
                .any(|probe| probe.subject == candidate.subject)
            {
                Some("a probe on this subject is already open".to_string())
            } else if plan
                .selected
                .iter()
                .any(|probe| probe.subject == candidate.subject)
            {
                Some("this pass already selected a probe on this subject".to_string())
            } else if self.open.len() + plan.selected.len() >= MAXIMUM_OPEN_PROBES {
                Some(format!(
                    "{MAXIMUM_OPEN_PROBES} probes are already open; settle one before opening another"
                ))
            } else if candidate.maximum_loss > ceiling {
                Some(format!(
                    "its maximum loss {} exceeds the per-probe ceiling {ceiling}; size the probe \
                     smaller or raise the mandate's exploration share",
                    candidate.maximum_loss
                ))
            } else if candidate.maximum_loss > remaining {
                Some(format!(
                    "the budget has {remaining} uncommitted and the probe would risk {}",
                    candidate.maximum_loss
                ))
            } else {
                None
            };
            match reason {
                Some(reason) => plan.declined.push(DeclinedProbe {
                    kind: candidate.kind,
                    subject: candidate.subject.clone(),
                    reason,
                }),
                None => {
                    sequence += 1;
                    remaining -= candidate.maximum_loss;
                    plan.committed_now += candidate.maximum_loss;
                    plan.selected.push(Probe {
                        id: format!("probe-{sequence}"),
                        kind: candidate.kind,
                        subject: candidate.subject.clone(),
                        maximum_loss: candidate.maximum_loss,
                        uncertainty_at_open: candidate.uncertainty,
                        opened_at: now,
                        expires_at: now.saturating_add(validity),
                        score,
                    });
                }
            }
        }
        Ok(plan)
    }

    /// The upper-confidence bound for one candidate. See [`Self::plan`] for
    /// the formula and the argument for it.
    fn score(&self, candidate: &ProbeCandidate) -> f64 {
        let measured = self.kinds.get(&candidate.kind);
        let (probed, gain) = measured.map_or((0.0, 0.0), |record| {
            // u64 → f64: counts of settlements, in the statistics lane.
            (record.probed as f64, record.probed_gain)
        });
        let expected_gain = (PRIOR_WEIGHT * candidate.uncertainty + gain) / (PRIOR_WEIGHT + probed);
        let attempts = f64::from(self.attempts(&candidate.subject));
        let bonus = EXPLORATION_CONSTANT
            * ((1.0 + self.opened_total as f64).ln() / (1.0 + attempts)).sqrt();
        candidate.value_weight() * (expected_gain + bonus)
    }

    /// Take up a plan: the selected probes become live and their bounds
    /// become committed capital.
    ///
    /// Refuses a plan that would commit more than the budget it was drawn
    /// against, and refuses one drawn against a book that has moved since —
    /// both fail closed, because a commitment accepted twice is the
    /// double-spend [`super::reservation`] exists to prevent, wearing a
    /// research label.
    pub fn open_plan(&mut self, plan: &ExplorationPlan) -> Result<usize> {
        if plan.committed_before != self.committed() {
            return Err(Error::denied(format!(
                "the plan was drawn when {} was committed and {} is committed now; re-plan \
                 against the book as it stands rather than opening a stale selection",
                plan.committed_before,
                self.committed()
            )));
        }
        if plan.committed_before + plan.committed_now > plan.budget {
            return Err(Error::denied(format!(
                "the plan commits {} against a budget of {}; nothing is opened",
                plan.committed_before + plan.committed_now,
                plan.budget
            )));
        }
        for probe in &plan.selected {
            if self.open.contains_key(&probe.id) {
                return Err(Error::denied(format!(
                    "{} is already open; a probe id is opened once",
                    probe.id
                )));
            }
        }
        for probe in &plan.selected {
            self.sequence = self.sequence.max(sequence_of(&probe.id));
            self.opened_total += 1;
            self.kinds.entry(probe.kind).or_default().opened += 1;
            self.bump_attempts(&probe.subject);
            self.open.insert(probe.id.clone(), probe.clone());
        }
        Ok(plan.selected.len())
    }

    /// Close a probe with what the subject now measures.
    ///
    /// The capital returns to the budget either way; what differs is what the
    /// book learned. An unknown id is refused rather than treated as already
    /// settled, because a settlement that "succeeds" against nothing turns a
    /// typo into a clean audit trail.
    pub fn settle(&mut self, id: &str, outcome: &ProbeOutcome) -> Result<ProbeSettlement> {
        let Some(probe) = self.open.remove(id) else {
            return Err(Error::not_found(format!(
                "no probe named {id} is open; a probe is settled once, and an expired one is \
                 still open until it is settled or abandoned"
            )));
        };
        let gain = probe.uncertainty_at_open - outcome.uncertainty_now;
        let within_bound = outcome.realised_cost <= probe.maximum_loss;
        let record = self.kinds.entry(probe.kind).or_default();
        match outcome.evidence {
            ProbeEvidence::Probed => {
                record.probed += 1;
                record.probed_gain += gain;
            }
            ProbeEvidence::Observed => record.observed += 1,
        }
        record.realised_cost += outcome.realised_cost;
        if !within_bound {
            record.bound_breaches += 1;
        }
        self.spend += outcome.realised_cost;
        self.settled_total += 1;
        Ok(ProbeSettlement {
            id: probe.id,
            kind: probe.kind,
            subject: probe.subject,
            evidence: outcome.evidence,
            realised_cost: outcome.realised_cost,
            information_gain: gain,
            within_bound,
        })
    }

    /// Close a probe that can no longer be measured, returning its capital.
    ///
    /// Separate from [`Self::settle`] and counted separately: a subject that
    /// vanished did not resolve to zero gain, and recording it as a
    /// settlement would put a measurement in the record where there was none.
    pub fn abandon(&mut self, id: &str, reason: &str) -> Result<Probe> {
        if reason.trim().is_empty() {
            return Err(Error::invalid(
                "abandoning a probe needs a reason; an unexplained withdrawal is a gap in the \
                 record where a decision was",
            ));
        }
        let Some(probe) = self.open.remove(id) else {
            return Err(Error::not_found(format!(
                "no probe named {id} is open to abandon"
            )));
        };
        self.abandoned_total += 1;
        Ok(probe)
    }

    /// Count an attempt against a subject, forgetting the least-probed
    /// subject when the map is full.
    fn bump_attempts(&mut self, subject: &str) {
        let entry = self.attempts.entry(subject.to_string()).or_insert(0);
        *entry = entry.saturating_add(1);
        while self.attempts.len() > MAXIMUM_TRACKED_SUBJECTS {
            // Ties break on subject order, so two replays forget the same one.
            let stalest = self
                .attempts
                .iter()
                .filter(|(key, _)| key.as_str() != subject)
                .min_by(|left, right| left.1.cmp(right.1).then_with(|| left.0.cmp(right.0)))
                .map(|(key, _)| key.clone());
            match stalest {
                Some(key) => {
                    self.attempts.remove(&key);
                    self.subjects_forgotten += 1;
                }
                None => break,
            }
        }
    }
}

/// The trailing number of a `probe-N` id, or zero for anything else.
fn sequence_of(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|tail| tail.parse::<u64>().ok())
        .unwrap_or(0)
}

/// The budget a mandate sets aside: a stated share of deployable capital.
///
/// Refuses rather than corrects, on both arguments. Negative capital is a
/// book that has not been reconciled, and a share outside `[0, 1]` is a
/// caller who confused percent with fraction — a share of 20 clamped to 1
/// would explore with the entire book while the caller believed it was
/// exploring with a fifth of it.
pub fn budget_for(deployable: Decimal, share: Decimal) -> Result<Decimal> {
    if deployable.is_negative() {
        return Err(Error::invalid(format!(
            "an exploration budget cannot be a share of negative capital ({deployable}); \
             reconcile the book first"
        )));
    }
    if share.is_negative() || share > Decimal::ONE {
        return Err(Error::invalid(format!(
            "an exploration share must be a fraction in [0, 1], not {share}; if this was a \
             percentage, divide by one hundred"
        )));
    }
    deployable.checked_mul(share).ok_or_else(|| {
        Error::numeric(format!(
            "the exploration budget {deployable} × {share} overflows the decimal range"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    fn candidate(kind: ProbeKind, subject: &str, uncertainty: f64) -> ProbeCandidate {
        ProbeCandidate::new(kind, subject, uncertainty, dec!("1000"), dec!("100"))
            .expect("a well-formed candidate")
    }

    #[test]
    fn a_share_above_one_is_refused_rather_than_clamped_to_the_whole_book() {
        let error = budget_for(dec!("1000000"), dec!("20")).expect_err("a share of 20 is refused");
        assert!(
            error.message().contains("divide by one hundred"),
            "the refusal does not say what to do instead: {}",
            error.message()
        );
        // The premise: the same capital at a real share does produce a budget,
        // so the refusal above is about the share and not about the capital.
        assert_eq!(
            budget_for(dec!("1000000"), dec!("0.02")).expect("a 2% share is a budget"),
            dec!("20000")
        );
    }

    #[test]
    fn a_candidate_with_nothing_unresolved_is_refused() {
        let error = ProbeCandidate::new(
            ProbeKind::UncertainModel,
            "detector:gap",
            0.0,
            dec!("1000"),
            dec!("100"),
        )
        .expect_err("an uncertainty of zero is refused");
        assert!(
            error.message().contains("nothing unresolved"),
            "the refusal does not name the reason: {}",
            error.message()
        );
    }

    #[test]
    fn a_probe_bounded_at_zero_is_refused_because_it_buys_nothing() {
        let error = ProbeCandidate::new(
            ProbeKind::CapacityAtSize,
            "strategy:mean-reversion",
            0.5,
            dec!("1000"),
            Decimal::ZERO,
        )
        .expect_err("a bound of zero is refused");
        assert!(
            error.message().contains("maximum loss"),
            "the refusal does not name the field: {}",
            error.message()
        );
    }

    #[test]
    fn the_rarely_probed_subject_wins_a_tie_on_uncertainty() {
        let mut book = ExplorationBook::new();
        let often = candidate(ProbeKind::UncertainModel, "analyst:often", 0.5);
        let seldom = candidate(ProbeKind::UncertainModel, "analyst:seldom", 0.5);
        // Give one subject a probe history: open a probe on it and settle it.
        let first = book
            .plan(
                dec!("1000"),
                std::slice::from_ref(&often),
                Duration::from_hours(1),
                now(),
            )
            .expect("a plan");
        book.open_plan(&first).expect("the plan opens");
        let id = first.selected[0].id.clone();
        book.settle(
            &id,
            &ProbeOutcome::new(ProbeEvidence::Observed, Decimal::ZERO, 0.5)
                .expect("a well-formed outcome"),
        )
        .expect("the probe settles");
        assert_eq!(
            book.attempts("analyst:often"),
            1,
            "the premise: one subject has been probed"
        );
        assert_eq!(book.attempts("analyst:seldom"), 0);

        let plan = book
            .plan(
                dec!("1000"),
                &[often, seldom],
                Duration::from_hours(1),
                now(),
            )
            .expect("a plan");
        assert_eq!(
            plan.selected.first().map(|probe| probe.subject.as_str()),
            Some("analyst:seldom"),
            "the upper-confidence bound did not favour the unprobed subject: {plan:?}"
        );
    }

    #[test]
    fn a_probe_that_would_risk_more_than_a_quarter_of_the_budget_is_declined_and_named() {
        let book = ExplorationBook::new();
        let greedy = ProbeCandidate::new(
            ProbeKind::UnfamiliarVenue,
            "venue:XPAR",
            0.8,
            dec!("10000"),
            dec!("400"),
        )
        .expect("a well-formed candidate");
        let plan = book
            .plan(dec!("1000"), &[greedy], Duration::from_hours(1), now())
            .expect("a plan");
        assert!(
            plan.selected.is_empty(),
            "the oversized probe was selected: {plan:?}"
        );
        assert_eq!(plan.declined.len(), 1);
        assert!(
            plan.declined[0].reason.contains("per-probe ceiling"),
            "the decline does not name the bound: {}",
            plan.declined[0].reason
        );
        // Not clamped to the ceiling: the probe the caller asked for is the
        // probe that was refused, and nothing smaller was substituted.
        assert_eq!(plan.committed_now, Decimal::ZERO);
    }

    #[test]
    fn observed_evidence_never_moves_the_measured_reward() {
        let mut book = ExplorationBook::new();
        let plan = book
            .plan(
                dec!("1000"),
                &[candidate(ProbeKind::StaleEstimate, "family:carry", 0.9)],
                Duration::from_hours(1),
                now(),
            )
            .expect("a plan");
        book.open_plan(&plan).expect("the plan opens");
        let id = plan.selected[0].id.clone();
        let settlement = book
            .settle(
                &id,
                &ProbeOutcome::new(ProbeEvidence::Observed, Decimal::ZERO, 0.1)
                    .expect("a well-formed outcome"),
            )
            .expect("the probe settles");
        // The premise: the gain is real and large, so a book that scored it
        // would show it.
        assert!(
            (settlement.information_gain - 0.8).abs() < 1e-9,
            "the gain was not measured: {settlement:?}"
        );
        let record = book
            .record(ProbeKind::StaleEstimate)
            .expect("the kind has a record");
        assert_eq!(record.observed, 1);
        assert_eq!(record.probed, 0);
        assert_eq!(
            record.measured_gain(),
            None,
            "observation was folded into the measured reward, which would report that probing \
             works on evidence that no probing happened"
        );
    }

    #[test]
    fn a_settlement_over_the_probes_own_bound_is_recorded_rather_than_refused() {
        let mut book = ExplorationBook::new();
        let plan = book
            .plan(
                dec!("1000"),
                &[candidate(
                    ProbeKind::CapacityAtSize,
                    "strategy:momentum",
                    0.4,
                )],
                Duration::from_hours(1),
                now(),
            )
            .expect("a plan");
        book.open_plan(&plan).expect("the plan opens");
        let id = plan.selected[0].id.clone();
        let settlement = book
            .settle(
                &id,
                &ProbeOutcome::new(ProbeEvidence::Probed, dec!("150"), 0.3)
                    .expect("a well-formed outcome"),
            )
            .expect("the probe settles");
        assert!(
            !settlement.within_bound,
            "a cost of 150 against a bound of 100 was recorded as within bound"
        );
        assert_eq!(book.spend(), dec!("150"), "the overspend left the account");
        assert_eq!(
            book.record(ProbeKind::CapacityAtSize)
                .map(|record| record.bound_breaches),
            Some(1)
        );
    }

    #[test]
    fn capital_returns_to_the_budget_when_a_probe_closes() {
        let mut book = ExplorationBook::new();
        let plan = book
            .plan(
                dec!("1000"),
                &[candidate(ProbeKind::UncertainModel, "detector:gap", 0.6)],
                Duration::from_hours(1),
                now(),
            )
            .expect("a plan");
        book.open_plan(&plan).expect("the plan opens");
        assert_eq!(
            book.committed(),
            dec!("100"),
            "the premise: capital is held"
        );
        book.abandon(&plan.selected[0].id, "the subject is no longer measured")
            .expect("the probe is abandoned");
        assert_eq!(book.committed(), Decimal::ZERO);
        assert_eq!(book.abandoned_total(), 1);
        assert_eq!(
            book.settled_total(),
            0,
            "an abandonment was counted as a measurement"
        );
    }

    #[test]
    fn a_plan_drawn_against_a_stale_book_is_refused() {
        let mut book = ExplorationBook::new();
        let plan = book
            .plan(
                dec!("1000"),
                &[candidate(ProbeKind::UncertainModel, "detector:gap", 0.6)],
                Duration::from_hours(1),
                now(),
            )
            .expect("a plan");
        book.open_plan(&plan).expect("the plan opens");
        let error = book
            .open_plan(&plan)
            .expect_err("the same plan cannot be opened twice");
        assert!(
            error.message().contains("re-plan"),
            "the refusal does not say what to do instead: {}",
            error.message()
        );
        assert_eq!(book.open_count(), 1, "the second open added a probe");
    }

    #[test]
    fn an_idle_plan_says_which_idle_state_it_is_in() {
        let book = ExplorationBook::new();
        let no_budget = book
            .plan(Decimal::ZERO, &[], Duration::from_hours(1), now())
            .expect("a plan over no budget");
        assert!(
            no_budget
                .summary()
                .contains("no mandate sets a share aside"),
            "the zero-budget summary does not say why: {}",
            no_budget.summary()
        );
        let no_candidates = book
            .plan(dec!("1000"), &[], Duration::from_hours(1), now())
            .expect("a plan over no candidates");
        assert!(
            no_candidates
                .summary()
                .contains("0 candidate(s) considered"),
            "the empty-candidate summary does not say so: {}",
            no_candidates.summary()
        );
        assert_ne!(
            no_budget.summary(),
            no_candidates.summary(),
            "two different idle states read identically"
        );
    }
}
