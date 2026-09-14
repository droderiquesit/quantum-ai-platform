//! Blueprint §12.3's fifth row — "a strategy family whose weight should be
//! revised" — built as far as the evidence honestly reaches, and no further.
//!
//! The row asks for two things: measure which families are earning their
//! funding, and revise the weights of the ones that are not. This module does
//! the first and **deliberately does not do the second**, and the absence is
//! the guarantee rather than an omission (ADR 0064).
//!
//! # Why no weight moves
//!
//! Every weight a family review could narrow sits behind a writer with no
//! production caller. `CentralPlane::issue` is the only writer of a capital
//! envelope and is reached from nothing but a test; `CentralPlane::set_proposal`
//! is reached only from `central::learning::resize`, which mutates a proposal
//! that must already exist; `optimization_engine::family_horizons` has no
//! caller outside its own suite. Verify rather than believe:
//!
//! ```text
//! grep -rn '\.issue(' backend/crates --include=*.rs
//! grep -rn 'set_proposal\|family_horizons' backend/crates --include=*.rs
//! ```
//!
//! A `FamilyCap` built on top of those today would be a control that cannot
//! fire — the shape this repository records under `MaxExpectedShortfall`, a
//! limit that shipped in every default set and could never trigger, and reads
//! as protection while being none. So the finding this module produces
//! carries no multiplier, no notional, no `Decimal` and no money type of any
//! kind, and no function exported here returns one — the acceptance scan
//! enumerates the return types this module may name rather than searching for
//! the word `Decimal`, because a newtype over one would pass that search.
//! A later edit cannot "wire it up" without first writing the weight it would
//! move.
//!
//! Be precise about the scope of that, because it is narrower than it reads
//! and a reviewer found the gap. [`FamilyStanding::deflated_excess`] is a
//! public `f64` and any caller can multiply by it. The guarantee is not "no
//! number leaves this module": it is that the *finding* carries none, and
//! that no exported *function* hands one out. The magnitude has to live
//! somewhere a person can read, and it lives on the measurement — beside the
//! member counts that qualify it — rather than on the record that names two
//! families, which is the field the obvious next edit would reach for. A
//! Sharpe excess in annualised units is also not a multiplier in any case: it
//! is not bounded to `(0, 1]`, it is routinely negative, and a caller that
//! scaled a notional by it would be inventing a unit conversion nobody
//! recorded.
//!
//! # The statistic, and why it is the gate's own
//!
//! A family's figure is the member mean of `observed - expected_maximum` from
//! [`deflated_sharpe`] — how far its holdout Sharpe stands above what its own
//! search alone would produce, in annualised Sharpe units.
//!
//! It is not [`qip_simulation_engine::validation::DeflatedSharpe::observed`],
//! which is the *undeflated* Sharpe: grading a family that tried ten thousand
//! configurations on the same scale as one that tried ten is exactly the
//! selection bias the holdout gate was built to correct, and re-introducing
//! it here would let a review recommend funding on a number the selector
//! never used.
//!
//! ## Why the count is passed in rather than read off the evidence
//!
//! [`qip_lifecycle::gates::HoldoutGate::deflated`] is the obvious call here,
//! and it cannot be made. It resolves the lifetime trial count from
//! `StrategyEvidence::trial_account`, and **no candidate in this platform ever
//! carries one**: `qip_lifecycle::ledger`'s `charge_holdout_trials` charges
//! the account into a `Cow` that `attempt_promotion` hands to the gate and
//! drops, and `StrategyFactory::submit_evidence`, the only writer that could
//! put it back, has no caller at all. Check both rather than believe this:
//!
//! ```text
//! grep -rn 'with_trial_account\|submit_evidence' backend/crates --include=*.rs
//! ```
//!
//! A review that called `HoldoutGate::deflated` would therefore refuse every
//! member of every family for ever, while reading in the code like a working
//! comparison — the `MaxExpectedShortfall` shape a second time, inside the
//! very lane that exists to refuse it. So the count is resolved by the caller
//! from the trial book (`TrialBook::lifetime_trials`, a read and not a
//! charge), and the deflation is the same [`deflated_sharpe`] call the gate's
//! own last line makes.
//!
//! ## The count is the family's search now, not the gate's snapshot
//!
//! Until 2026-09-14 the paragraph above ended "…which is the same number
//! `HoldoutGate::charged_trials` resolves to". **It is not the same number**,
//! and the test that was offered as proof held only at the one arity where
//! the two coincide. The two quantities:
//!
//! - `HoldoutGate::charged_trials`, which `HoldoutGate::deflated` calls, returns
//!   `TrialAccount::lifetime()` — the family's total *as it stood when that
//!   member was charged*. A snapshot, frozen at one member's evaluation.
//! - `TrialBook::lifetime_trials` returns the last
//!   journal record's `lifetime_after` — the family's total *now*, which
//!   every later sibling's charge has raised.
//!
//! They agree only for the member charged last, and therefore for every
//! member of a one-member family. Enrol two and they diverge: the shipped
//! fixture, moved from one member to two, printed `left: 12` (the gate's
//! snapshot for the first member) against `right: 24` (the family's total
//! after the second was charged).
//!
//! **The family's total as of the review is the right quantity and the
//! arithmetic stays.** A deflated Sharpe exists to correct a result for the
//! multiple testing that produced it, and a comparison made *now* between two
//! families must correct each for the search it has actually done. Grading
//! family A on the twelve configurations it had tried when its first member
//! was evaluated, while family B is graded on ten thousand, re-introduces on
//! the review's own axis exactly the selection bias the deflation exists to
//! remove. The gate's snapshot is right for the gate — it decides one
//! admission at one instant — and wrong for a cross-family comparison drawn
//! afterwards.
//!
//! ### What that costs: a member's contribution is not stationary
//!
//! It follows, and is stated here because it is easy to discover later and
//! read as a bug: evaluating any sibling raises the family's lifetime count
//! and therefore lowers every other member's excess. Two
//! [`FamilyAllocationReview`] records built from byte-identical evidence at
//! two different cycles will disagree, legitimately.
//!
//! That is not a break with "every decision reproducible from the log alone".
//! The record carries its `cycle` and its `at`, and what it claims is the
//! family's standing *against the search as of that cycle* — a statement
//! about a moment, which replays to the same value from the same log because
//! the trial book is itself a hash-chained journal whose totals are
//! reconstructed rather than remembered. What is not reproducible is the
//! comparison of one member's figure across cycles, and nothing in this
//! module or the record it writes invites that comparison: the finding names
//! families, never members.
//!
//! `a_family_s_figure_is_the_gate_s_own_deflation_and_not_the_raw_sharpe`
//! keeps the degenerate one-member case, where the two counts coincide and
//! bit equality is a real claim about the arithmetic;
//! `a_family_s_count_is_the_whole_family_s_search_and_not_one_member_s_snapshot`
//! holds the general relation at two members — the review's count is the
//! family's current total, it is greater than or equal to every member's
//! snapshot, and equality falls on the member charged last.
//!
//! Both sides of the comparison are the same measure. A realised return over
//! a grant and a backtested holdout series are different quantities; a
//! comparison across them would be a finding about regime wearing a finding
//! about allocation's clothes.

use crate::platform::COUNTERFACTUAL_SIZING_MIN_SAMPLE;
use qip_contracts::gate::GateStage;
use qip_contracts::signal::StrategyId;
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_lifecycle::evidence::StrategyEvidence;
use qip_lifecycle::trials::StrategyFamily;
use qip_observability::metrics::{Metrics, labels, names};
use qip_simulation_engine::validation::deflated_sharpe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Members whose evidence the gate could actually read, on *each* side of the
/// comparison, before a difference between two families is a finding rather
/// than noise.
///
/// **Evaluated members, not registered ones.** This bar tested
/// [`FamilyStanding::members`] until 2026-09-14, so a family of ten
/// registrations with one readable holdout series cleared a bar whose own
/// documentation says it exists "so a difference between two families is a
/// finding rather than noise" — on one observation. The constant is
/// [`COUNTERFACTUAL_SIZING_MIN_SAMPLE`], a *sample* bar, and it was being
/// applied to a *population* count; a registration nothing could deflate is
/// not an observation, and counting it as one is the same error as averaging
/// a refused member in as a zero, which [`standings`] already refuses to do
/// three lines from here.
///
/// [`COUNTERFACTUAL_SIZING_MIN_SAMPLE`], by reference, for the reason
/// `rule_review`, `sizing_review` and `venue_review` all give: the platform
/// has one answer to how much evidence makes a pattern a finding, and three
/// numbers that happen to agree today would not stay agreed.
pub const FAMILY_REVIEW_MIN_MEMBERS: usize = COUNTERFACTUAL_SIZING_MIN_SAMPLE;

/// How far an unfunded family's deflated excess must stand above the best
/// funded family's before the gap is a finding: half a unit of annualised
/// Sharpe.
///
/// One auditable number, as `SIZING_CAP_MULTIPLIER` is, rather than a curve
/// fitted after the fact. Half a Sharpe is roughly the width of the band the
/// holdout gate itself treats as the difference between a result and a
/// coincidence, and a margin smaller than that would make a finding out of
/// the estimation error in the two means.
pub const FAMILY_REVIEW_MARGIN: f64 = 0.5;

/// The finding stands and is on the record.
pub const FAMILY_FINDING_PROPOSED: &str = "proposed";
/// The evidence stopped clearing the bar and the finding is withdrawn.
pub const FAMILY_FINDING_WITHDRAWN: &str = "withdrawn";

/// One registered candidate, as the review reads it.
///
/// A borrowed view rather than a tuple, so a caller cannot transpose the
/// stage and the family without the compiler noticing, and so the list the
/// platform passes is a projection of the foundry's population rather than a
/// second copy of it.
#[derive(Clone, Copy, Debug)]
pub struct FamilyMember<'a> {
    pub family: &'a StrategyFamily,
    pub strategy: &'a StrategyId,
    /// The rung the factory's ledger says it stands at.
    pub stage: GateStage,
    pub evidence: &'a StrategyEvidence,
    /// The family's lifetime trial count, from the trial book the factory
    /// enrolled this strategy in — the number the deflation corrects
    /// against. One value for the whole family, as the search stands at the
    /// review, and deliberately **not** the snapshot the gate charged this
    /// member under: see the module documentation for both halves — why it is
    /// passed rather than read from `evidence.trial_account`, which is `None`
    /// on every candidate this platform registers, and why the family's
    /// current total rather than the member's snapshot is the right
    /// correction for a comparison drawn now.
    pub lifetime_trials: u64,
}

/// Where one family stands: how many of its members hold capital, how much of
/// its evidence could be read, and what the gate's deflation says about it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilyStanding {
    pub family: String,
    /// Registered members, whatever rung they stand at.
    pub members: usize,
    /// Members at a rung that may hold capital
    /// ([`GateStage::holds_capital`]). Zero is the state every deployment of
    /// this platform is in today, and the review says so rather than
    /// inferring it.
    pub funded: usize,
    /// Members whose holdout evidence the gate could actually deflate.
    pub admitted: usize,
    /// Members whose evidence the gate refused — no holdout series, no trial
    /// account, a series too short to deflate, or a result that is not a
    /// finite number. Counted rather than dropped, because a family with ten
    /// members and one readable one is not a family with one member, and a
    /// standing that hid the difference would invite exactly that reading.
    pub refused: usize,
    /// Member mean of `observed - expected_maximum` over the admitted
    /// members; zero where none was admitted. See the module documentation
    /// for why this is not the observed Sharpe.
    pub deflated_excess: f64,
}

impl FamilyStanding {
    /// Whether any member of this family stands at a capital-holding rung.
    pub fn is_funded(&self) -> bool {
        self.funded > 0
    }

    /// Whether this family has enough *evaluated* members for a difference
    /// against another to be read as a pattern.
    ///
    /// [`Self::admitted`] and never [`Self::members`]. The figure being
    /// compared is a mean over the admitted members alone, so the sample
    /// behind it is `admitted`; a family with ten registrations and one
    /// readable series has produced one observation, and testing `members`
    /// let that clear a ten-observation bar. Fixed 2026-09-14 —
    /// `a_family_registered_ten_times_and_read_once_has_one_observation_and_not_ten`
    /// is the test.
    fn clears_the_evidence_bar(&self) -> bool {
        self.admitted >= FAMILY_REVIEW_MIN_MEMBERS
    }
}

/// Group the registered population by family and score each one.
///
/// Pure, and a [`BTreeMap`] because the order reaches the journal record and
/// the record is replayed: a review that reordered its families between two
/// replays of one log would produce two different lines for one fact.
///
pub fn standings(members: &[FamilyMember<'_>]) -> BTreeMap<String, FamilyStanding> {
    // Sums kept beside the standings rather than on them: a running total
    // with a divisor is not a standing, and a field carrying one would be
    // readable as a weight by anyone who did not read this comment.
    let mut excess: BTreeMap<String, f64> = BTreeMap::new();
    let mut out: BTreeMap<String, FamilyStanding> = BTreeMap::new();
    for member in members {
        let name = member.family.as_str().to_string();
        let standing = out.entry(name.clone()).or_insert_with(|| FamilyStanding {
            family: name.clone(),
            members: 0,
            funded: 0,
            admitted: 0,
            refused: 0,
            deflated_excess: 0.0,
        });
        standing.members += 1;
        if member.stage.holds_capital() {
            standing.funded += 1;
        }
        // usize/GateStage above, f64 below: this is the crossing point from
        // the lifecycle's counts into the statistics lane, and everything
        // past it is `f64` because a Sharpe ratio is not money.
        match member_deflation(member) {
            Ok(deflated) => {
                let member_excess = deflated.observed - deflated.expected_maximum;
                if member_excess.is_finite() {
                    standing.admitted += 1;
                    *excess.entry(name).or_insert(0.0) += member_excess;
                } else {
                    // Refused rather than counted as zero. A family whose
                    // arithmetic did not produce a number has not produced a
                    // zero, and averaging one in would drag a real family's
                    // figure toward a value nobody computed.
                    standing.refused += 1;
                }
            }
            Err(_) => standing.refused += 1,
        }
    }
    for (name, standing) in out.iter_mut() {
        if standing.admitted > 0 {
            let total = excess.get(name).copied().unwrap_or(0.0);
            standing.deflated_excess = total / standing.admitted as f64;
        }
    }
    out
}

/// One member's deflated Sharpe, by the same arithmetic
/// [`qip_lifecycle::gates::HoldoutGate::deflated`] ends in.
///
/// Refuses — rather than scoring zero — a member with no holdout series and a
/// lifetime count that does not fit a `usize`. `deflated_sharpe` makes the
/// rest of the refusals itself: too few observations, no trials charged,
/// returns that do not vary.
fn member_deflation(
    member: &FamilyMember<'_>,
) -> qip_core::error::Result<qip_simulation_engine::validation::DeflatedSharpe> {
    let holdout = member.evidence.holdout.as_ref().ok_or_else(|| {
        qip_core::error::Error::invalid(format!(
            "{} submitted no holdout evidence, so its family's figure has nothing to read",
            member.strategy
        ))
    })?;
    let trials = usize::try_from(member.lifetime_trials).map_err(|_| {
        qip_core::error::Error::numeric(format!(
            "a lifetime count of {} trials cannot be deflated against",
            member.lifetime_trials
        ))
    })?;
    deflated_sharpe(&holdout.holdout_returns, trials, holdout.periods_per_year)
}

/// A pair of families the evidence separates: one holding capital, one not,
/// with the unfunded one ahead.
///
/// No field on this type is a weight, a multiplier, a notional or a fraction,
/// and none is a [`qip_core::Decimal`]. That is the whole of the platform's
/// response to §12.3's fifth row, and it is held by there being no type here
/// that could carry anything else — not by care taken at a call site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misallocation {
    /// The family whose evidence is ahead and whose members hold no capital.
    pub unfunded: String,
    /// The best-placed funded family it stands above.
    pub funded: String,
    /// How many of the unfunded family's members the gate could actually
    /// deflate — the sample the figure was computed over, and never the
    /// number of registrations.
    pub unfunded_evaluated: usize,
    /// The same for the funded side.
    pub funded_evaluated: usize,
}

/// The strongest funded/unfunded separation in `standings`, if one clears
/// both bars.
///
/// The comparison is against the **best** funded family rather than any
/// funded family, which is the strict reading: "this family is ahead of
/// everything we are paying for" is a finding, while "this family is ahead of
/// the worst thing we are paying for" is true of almost any population and
/// would fire every cycle.
///
/// Ties are broken by name, and the map's own order decides them, so two
/// replays of one log name the same pair.
pub fn misallocation(standings: &BTreeMap<String, FamilyStanding>) -> Option<Misallocation> {
    let best_unfunded = best(
        standings
            .values()
            .filter(|standing| !standing.is_funded() && standing.clears_the_evidence_bar()),
    )?;
    let best_funded = best(
        standings
            .values()
            .filter(|standing| standing.is_funded() && standing.clears_the_evidence_bar()),
    )?;
    // The separate `admitted > 0` guard that used to sit beside the bar is
    // gone because the bar is now on `admitted` itself and
    // `FAMILY_REVIEW_MIN_MEMBERS` is ten. It is not missing: a second, weaker
    // readability test beside a stronger one reads as though the stronger one
    // did not cover it, which is how the two drifted apart in the first
    // place.
    //
    // `>=` and not `>`: the margin is the bar, and a gap that lands exactly
    // on a declared threshold clears it. The same shape as
    // `SizeRegret::clears`. Held by
    // `a_gap_of_exactly_the_margin_is_a_finding_and_one_ulp_below_it_is_not`,
    // because an argued boundary nothing tests is an argument and not a bar.
    if best_unfunded.deflated_excess - best_funded.deflated_excess >= FAMILY_REVIEW_MARGIN {
        Some(Misallocation {
            unfunded: best_unfunded.family.clone(),
            funded: best_funded.family.clone(),
            unfunded_evaluated: best_unfunded.admitted,
            funded_evaluated: best_funded.admitted,
        })
    } else {
        None
    }
}

/// The highest `deflated_excess` in an already-ordered iterator, keeping the
/// first on a tie.
///
/// Written as a fold with a strict `>` rather than `max_by` with a partial
/// comparison, because `f64` has no total order and `partial_cmp` on a pair
/// including a NaN would decide the winner by iteration order. Non-finite
/// excesses cannot reach here — `standings` refuses them — and this is
/// written so that would still be true if that changed.
fn best<'a>(
    mut candidates: impl Iterator<Item = &'a FamilyStanding>,
) -> Option<&'a FamilyStanding> {
    let mut leader = candidates.next()?;
    for candidate in candidates {
        if candidate.deflated_excess > leader.deflated_excess {
            leader = candidate;
        }
    }
    Some(leader)
}

/// Write the standings gauge, on every cycle and **including when both arms
/// are zero**.
///
/// The zero is the point. Blueprint §12.3's fifth row has never had a
/// subject in any deployment of this platform, because nothing funds a
/// family; a `funded` arm that reads zero every cycle states that as a fact
/// somebody can chart, where an absent series reads identically to a review
/// that never ran. `RULE_DORMANT` takes the same discipline for the same
/// reason.
///
/// Records counts of families and never a family name: the foundry mints as
/// many families as a sweep produces, and a label keyed on one would be
/// unbounded. The two label values are fixed here.
pub fn record_standings(metrics: &Metrics, standings: &BTreeMap<String, FamilyStanding>) {
    let funded = standings
        .values()
        .filter(|standing| standing.is_funded())
        .count();
    let unfunded = standings.len() - funded;
    // usize → f64 at the metric boundary: a gauge is an f64 and a family
    // count is small enough that the conversion is exact.
    metrics.gauge(
        names::FAMILY_STANDINGS,
        labels([("standing", "funded")]),
        funded as f64,
    );
    metrics.gauge(
        names::FAMILY_STANDINGS,
        labels([("standing", "unfunded")]),
        unfunded as f64,
    );
}

/// The measurement, journaled once a cycle: where every registered family
/// stands, in name order.
///
/// A record of what was measured, not of what was decided, because nothing
/// was decided. The per-family figures live here rather than on
/// [`MisallocationFinding`] so that the finding stays a pointer at two names
/// and never acquires a number a later edit could read as a size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilyAllocationReview {
    /// One entry per registered family, in name order.
    pub standings: Vec<FamilyStanding>,
    pub families: usize,
    /// Families with at least one member at a capital-holding rung.
    pub funded_families: usize,
    pub cycle: u64,
    pub at: Timestamp,
}

impl FamilyAllocationReview {
    /// Assemble the record from the standings the cycle computed.
    pub fn of(standings: &BTreeMap<String, FamilyStanding>, cycle: u64, at: Timestamp) -> Self {
        let entries: Vec<FamilyStanding> = standings.values().cloned().collect();
        let funded_families = entries
            .iter()
            .filter(|standing| standing.is_funded())
            .count();
        Self {
            families: entries.len(),
            funded_families,
            standings: entries,
            cycle,
            at,
        }
    }

    /// One line for the cycle's detail.
    ///
    /// Names the families rather than only counting them, because the
    /// question this row exists to answer is *which* families the desk is
    /// paying for, and a count answers it for nobody. Bounded by the number
    /// of sweeps the foundry has run, which is small and set by the desk —
    /// unlike a per-instrument line, which is why the sizing review counts
    /// where this names. Name order, so two replays of one log produce one
    /// line.
    pub fn describe(&self) -> String {
        let named: Vec<String> = self
            .standings
            .iter()
            .map(|standing| {
                format!(
                    "{} ({} member(s), {} funded)",
                    standing.family, standing.members, standing.funded
                )
            })
            .collect();
        format!(
            "{} strategy family(ies) reviewed against funding standing, {} of them funded [{}]; \
             the review allocates nothing",
            self.families,
            self.funded_families,
            named.join(", ")
        )
    }
}

impl EventBody for FamilyAllocationReview {
    const TOPIC: Topic = Topic::FamilyAllocationReviewed;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        // `journal_once` prefixes every key with the topic name, and this
        // topic carries two bodies, so each needs a prefix of its own or a
        // review and a finding from one cycle would collide on the log.
        Some(format!("family-review:{}", self.cycle))
    }
}

/// A family the evidence says is ahead of everything being funded, put on the
/// record.
///
/// Carries two names and two counts. It carries no margin, no excess and no
/// ratio, deliberately: the magnitude is in the [`FamilyAllocationReview`]
/// beside it, and a number on *this* record is the one field the obvious next
/// edit — "size the reallocation by how far ahead it is" — would reach for.
/// §12.4's guardrail against loosening a control on counterfactual evidence
/// is held here the way ADR 0061 and ADR 0063 hold it: by there being no code
/// path that widens anything, rather than by nobody having written one yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MisallocationFinding {
    pub unfunded: String,
    pub funded: String,
    /// The sample each side's figure was computed over: members the gate
    /// could read, not members registered. Named `unfunded_members` until
    /// 2026-09-14, when it carried the registration count — a reader takes
    /// this number for the sample size, and on a family of ten registrations
    /// with one readable series it said ten.
    pub unfunded_evaluated: usize,
    pub funded_evaluated: usize,
    /// [`FAMILY_FINDING_PROPOSED`] or [`FAMILY_FINDING_WITHDRAWN`].
    pub outcome: String,
    pub cycle: u64,
    pub at: Timestamp,
}

impl MisallocationFinding {
    pub fn of(pair: &Misallocation, outcome: &str, cycle: u64, at: Timestamp) -> Self {
        Self {
            unfunded: pair.unfunded.clone(),
            funded: pair.funded.clone(),
            unfunded_evaluated: pair.unfunded_evaluated,
            funded_evaluated: pair.funded_evaluated,
            outcome: outcome.to_string(),
            cycle,
            at,
        }
    }

    /// The key a withdrawal is journaled under, given the pair that was open.
    pub fn describe(&self) -> String {
        format!(
            "family {} holds no capital and its deflated evidence stands at least \
             {FAMILY_REVIEW_MARGIN} above funded family {}; a finding and never a weight",
            self.unfunded, self.funded
        )
    }
}

impl EventBody for MisallocationFinding {
    const TOPIC: Topic = Topic::FamilyAllocationReviewed;
    /// Still `1` after `unfunded_members`/`funded_members` were renamed to
    /// `unfunded_evaluated`/`funded_evaluated` on 2026-09-14, and the argument
    /// is recorded here rather than only in the commit that did it, because
    /// this constant is where the next person will look.
    ///
    /// A version exists so that a reader meeting an older record knows which
    /// shape it is. Nothing is deployed and no process has ever written one of
    /// these records, so no stored bytes carry the old field names and none
    /// can be misdecoded. The only readers are in this workspace, and a rename
    /// makes each of them fail to *compile* rather than silently decode a
    /// field that is no longer there — which is the failure a version bump
    /// buys, already bought by the type system.
    ///
    /// That argument expires the moment one of these records is written by a
    /// deployed process. Renaming a field after that is a bump, and a reader
    /// of both versions.
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        // Injective on its inputs with no free-form segment: both interior
        // segments are `StrategyFamily` names, and `StrategyFamily::new`
        // already refuses `:` — for exactly this reason, stated in its own
        // refusal — so no two distinct pairs can spell one key.
        Some(format!(
            "family-finding:{}:{}:{}:{}",
            self.unfunded, self.funded, self.outcome, self.cycle
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::Duration;
    use qip_lifecycle::evidence::{
        CrossValidationRun, FeatureTiming, HoldoutEvidence, LeakageAudit,
    };
    use qip_lifecycle::gates::HoldoutGate;
    use qip_lifecycle::trials::TrialBook;
    use qip_numerics::stats;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    /// A deterministic return series whose Sharpe rises with `drift`.
    fn returns(seed: u64, count: usize, drift: f64) -> Vec<f64> {
        let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let u = ((state >> 33) as f64 / f64::from(u32::MAX)) - 0.5;
                drift + u * 0.01
            })
            .collect()
    }

    fn holdout(seed: u64, drift: f64, trials: usize) -> HoldoutEvidence {
        let observations = 300;
        HoldoutEvidence {
            holdout_returns: returns(seed, observations, drift),
            in_sample_folds: Vec::new(),
            out_of_sample_folds: Vec::new(),
            trials,
            periods_per_year: 252.0,
            cross_validation: CrossValidationRun {
                folds: 5,
                label_horizon: 10,
                embargo: 5,
                observations,
                purged: 0,
                embargoed: 0,
            },
            leakage: LeakageAudit {
                timings: vec![FeatureTiming {
                    feature: "feature-0".to_string(),
                    known_at: at(),
                    used_at: at().saturating_add(Duration::from_hours(1)),
                }],
                restated_without_snapshots: Vec::new(),
            },
        }
    }

    /// A population builder: the book that mints trial accounts, and the
    /// evidence each member carries.
    struct Population {
        book: TrialBook,
        families: Vec<StrategyFamily>,
        strategies: Vec<StrategyId>,
        stages: Vec<GateStage>,
        evidence: Vec<StrategyEvidence>,
    }

    impl Population {
        fn new() -> Self {
            Self {
                book: TrialBook::in_memory(),
                families: Vec::new(),
                strategies: Vec::new(),
                stages: Vec::new(),
                evidence: Vec::new(),
            }
        }

        /// Enrol `count` members of `family`, each carrying the same holdout
        /// series, at `stage`.
        fn enrol(mut self, family: &str, count: usize, stage: GateStage, drift: f64) -> Self {
            let family = StrategyFamily::new(family).expect("an admissible family name");
            if self.book.lifetime_trials(&family).is_none() {
                self.book.open_family(&family, at()).expect("family opened");
            }
            for index in 0..count {
                let strategy = StrategyId::new(format!("{}-{index}", family.as_str()));
                self.book
                    .enrol(&strategy, &family, at())
                    .expect("enrolled in its family");
                let evidence = holdout(1, drift, 12);
                let account = self
                    .book
                    .charge(&strategy, evidence.trials, at())
                    .expect("charged");
                self.families.push(family.clone());
                self.strategies.push(strategy);
                self.stages.push(stage);
                self.evidence.push(
                    StrategyEvidence::new()
                        .with_holdout(evidence)
                        .with_trial_account(account),
                );
            }
            self
        }

        /// Enrol `count` members of `family` that carry no holdout series at
        /// all, at `stage`.
        ///
        /// Registered and unreadable: `member_deflation` refuses each one and
        /// `standings` counts it under `refused`, which is the population a
        /// bar tested on `members` rather than `admitted` could not tell from
        /// a population of real observations.
        fn enrol_unreadable(mut self, family: &str, count: usize, stage: GateStage) -> Self {
            let family = StrategyFamily::new(family).expect("an admissible family name");
            if self.book.lifetime_trials(&family).is_none() {
                self.book.open_family(&family, at()).expect("family opened");
            }
            for index in 0..count {
                let strategy = StrategyId::new(format!("{}-blind-{index}", family.as_str()));
                self.book
                    .enrol(&strategy, &family, at())
                    .expect("enrolled in its family");
                self.families.push(family.clone());
                self.strategies.push(strategy);
                self.stages.push(stage);
                self.evidence.push(StrategyEvidence::new());
            }
            self
        }

        fn members(&self) -> Vec<FamilyMember<'_>> {
            (0..self.families.len())
                .map(|i| FamilyMember {
                    family: &self.families[i],
                    strategy: &self.strategies[i],
                    stage: self.stages[i],
                    evidence: &self.evidence[i],
                    // The book's own count, exactly as `Platform::family_standings`
                    // resolves it.
                    lifetime_trials: self
                        .book
                        .lifetime_trials(&self.families[i])
                        .expect("the family is open in the book"),
                })
                .collect()
        }
    }

    /// Drifts chosen so the unfunded family's deflated excess clears the
    /// funded one's by more than the margin. Asserted, not assumed, wherever
    /// it is relied on.
    const AHEAD: f64 = 0.0040;
    const BEHIND: f64 = 0.0004;

    #[test]
    fn a_family_whose_evidence_beats_every_funded_family_moves_no_weight_anywhere() {
        // The lane's whole claim in one test: the finding IS produced, and
        // producing it changes nothing that sizes a position. The premise is
        // asserted first because a test that only checked "nothing moved"
        // would pass on a review that found nothing at all.
        let population = Population::new()
            .enrol(
                "alpha",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Holdout,
                AHEAD,
            )
            .enrol(
                "omega",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Scaled,
                BEHIND,
            );
        let members = population.members();
        let standings = standings(&members);
        let finding = misallocation(&standings).expect("the premise: a finding is produced");
        assert_eq!(finding.unfunded, "alpha");
        assert_eq!(finding.funded, "omega");

        // And now what it did not do. This module exports no function that
        // returns a weight, so the assertion has to be made against the
        // record the finding produces: every field on it is a name, a count
        // or a timestamp, and none is a number a caller could size with.
        let record = MisallocationFinding::of(&finding, FAMILY_FINDING_PROPOSED, 7, at());
        let encoded = serde_json::to_value(&record).expect("the record serialises");
        let object = encoded.as_object().expect("a JSON object");
        assert_eq!(
            object.len(),
            7,
            "a field has been added to the misallocation finding; if it is a number, it is a \
             weight in waiting"
        );
        for (field, value) in object {
            if let Some(number) = value.as_f64() {
                assert!(
                    number.fract() == 0.0,
                    "field {field} on the misallocation finding is a fractional number; the one \
                     thing this record may never carry is a size"
                );
            }
        }
        // The standing beside it carries the magnitude, and it is a Sharpe
        // excess rather than anything in `(0, 1]` that a caller could
        // multiply a notional by.
        let alpha = standings.get("alpha").expect("alpha stands");
        assert!(
            alpha.deflated_excess > FAMILY_REVIEW_MARGIN,
            "the premise: alpha's excess clears the margin on its own"
        );
    }

    #[test]
    fn a_funded_family_that_outperforms_every_unfunded_one_produces_no_finding_at_all() {
        // The mirror. The review looks in one direction only — a family that
        // holds no capital and is ahead — and the opposite pattern, however
        // overwhelming, produces nothing. There is no "reduce the funded
        // family" arm, because reducing one is moving a weight.
        let population = Population::new()
            .enrol(
                "alpha",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Holdout,
                BEHIND,
            )
            .enrol("omega", FAMILY_REVIEW_MIN_MEMBERS, GateStage::Scaled, AHEAD);
        let members = population.members();
        let standings = standings(&members);

        // Both premises first: both sides cleared the member minimum and both
        // were readable, so `None` below is a verdict and not a shortfall.
        let alpha = standings.get("alpha").expect("alpha stands");
        let omega = standings.get("omega").expect("omega stands");
        assert_eq!(alpha.members, FAMILY_REVIEW_MIN_MEMBERS);
        assert_eq!(omega.members, FAMILY_REVIEW_MIN_MEMBERS);
        assert_eq!(alpha.admitted, FAMILY_REVIEW_MIN_MEMBERS);
        assert_eq!(omega.admitted, FAMILY_REVIEW_MIN_MEMBERS);
        assert!(!alpha.is_funded());
        assert!(omega.is_funded());
        assert!(
            omega.deflated_excess - alpha.deflated_excess >= FAMILY_REVIEW_MARGIN,
            "the premise: the funded family is ahead by more than the margin, in the direction \
             the review does not look"
        );

        assert_eq!(misallocation(&standings), None);
    }

    #[test]
    fn below_the_member_minimum_a_dominant_unfunded_family_is_not_a_finding() {
        // One short of the bar with an overwhelming excess: no finding. Then
        // one more member, unchanged in every other respect: a finding. The
        // admitting half is not optional — without it this test passes just
        // as well against a `misallocation` that returns `None` for every
        // input, which is a bar that is not a bar.
        let short = Population::new()
            .enrol(
                "alpha",
                FAMILY_REVIEW_MIN_MEMBERS - 1,
                GateStage::Holdout,
                AHEAD,
            )
            .enrol(
                "omega",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Scaled,
                BEHIND,
            );
        let short_members = short.members();
        let short_standings = standings(&short_members);
        let alpha = short_standings.get("alpha").expect("alpha stands");
        assert_eq!(
            alpha.members,
            FAMILY_REVIEW_MIN_MEMBERS - 1,
            "the premise: exactly one short of the bar"
        );
        assert!(
            alpha.deflated_excess
                - short_standings
                    .get("omega")
                    .expect("omega stands")
                    .deflated_excess
                >= FAMILY_REVIEW_MARGIN,
            "the premise: the excess would clear the margin if the members did"
        );
        assert_eq!(
            misallocation(&short_standings),
            None,
            "a family one member short of the bar produced a finding"
        );

        let full = Population::new()
            .enrol(
                "alpha",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Holdout,
                AHEAD,
            )
            .enrol(
                "omega",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Scaled,
                BEHIND,
            );
        let full_members = full.members();
        let full_standings = standings(&full_members);
        let finding = misallocation(&full_standings)
            .expect("one more member and the same evidence is a finding");
        assert_eq!(finding.unfunded_evaluated, FAMILY_REVIEW_MIN_MEMBERS);
    }

    #[test]
    fn a_family_with_no_funded_member_anywhere_in_the_population_produces_no_finding_and_says_so_in_the_series()
     {
        // The state every deployment of this platform is actually in, and the
        // only one §12.3's fifth row has ever been in: families exist,
        // nothing funds any of them, so there is no funded side to compare
        // against and no finding to make.
        //
        // The series is the deliverable here, not the silence. A gauge that
        // simply went unwritten when nothing was funded would read in every
        // chart exactly like a review that never ran, and "this row has never
        // had a subject" is the fact the lane exists to record.
        let population = Population::new()
            .enrol(
                "alpha",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Holdout,
                AHEAD,
            )
            .enrol("beta", FAMILY_REVIEW_MIN_MEMBERS, GateStage::Shadow, BEHIND);
        let members = population.members();
        let standings = standings(&members);

        assert_eq!(
            standings.len(),
            2,
            "the premise: the population is not empty"
        );
        for standing in standings.values() {
            assert_eq!(
                standing.funded, 0,
                "the premise: {} was funded, so this is not the no-subject case",
                standing.family
            );
        }
        assert_eq!(misallocation(&standings), None);

        let metrics = Metrics::new("family-review-test");
        record_standings(&metrics, &standings);
        let snapshot = metrics.snapshot();
        assert_eq!(
            snapshot.gauge(names::FAMILY_STANDINGS, &labels([("standing", "funded")])),
            Some(0.0),
            "the funded arm was not written as zero; an absent series and a zero read the same \
             on a chart and mean opposite things"
        );
        assert_eq!(
            snapshot.gauge(names::FAMILY_STANDINGS, &labels([("standing", "unfunded")])),
            Some(2.0)
        );
    }

    #[test]
    fn a_family_s_figure_is_the_gate_s_own_deflation_and_not_the_raw_sharpe() {
        // The test that stops the review grading the selector on a number the
        // selector never used. `DeflatedSharpe::observed` is the *undeflated*
        // Sharpe; the deflation is what it stands above. A review keyed on
        // `observed` would rank a family that tried ten thousand
        // configurations level with one that tried ten.
        let population = Population::new().enrol("alpha", 1, GateStage::Holdout, AHEAD);
        let members = population.members();
        let standings = standings(&members);
        let alpha = standings.get("alpha").expect("alpha stands");
        assert_eq!(
            alpha.admitted, 1,
            "the premise: one admitted member, so the mean is exact"
        );

        // Put the *gate* in front of the same evidence. `Population` charges a
        // real account through `TrialBook`, so this is the number an admission
        // would have been decided on, and the review's figure has to equal it
        // — though the review reached it through the book, because no
        // candidate this platform registers carries an account at all.
        //
        // One member on purpose, and the arity is the point rather than a
        // convenience. The gate deflates against the family's total *as it
        // stood when this member was charged* and the review against the
        // family's total *now*; in a one-member family those are the same
        // number, so here — and only here — bit equality is a claim about the
        // arithmetic instead of a coincidence of arity. The module doc
        // asserted the two counts were always equal until 2026-09-14, and
        // this test was the evidence offered; moving the fixture to two
        // members printed `left: 12`, `right: 24`. The general relation is
        // `a_family_s_count_is_the_whole_family_s_search_and_not_one_member_s_snapshot`
        // below, which is where a reader should go before believing anything
        // about the two counts at any other arity.
        let deflated = HoldoutGate::default()
            .deflated(members[0].strategy, members[0].evidence)
            .expect("the gate reads the same evidence");
        assert_eq!(
            deflated.trials, members[0].lifetime_trials as usize,
            "the premise: at one member the gate's snapshot and the family's current total are              the same count, which is what makes the bit equality below meaningful"
        );
        // Compared as bits rather than as floats. "To the bit" is the claim
        // — that the review reads the gate's arithmetic rather than an
        // arithmetic that agrees with it to a tolerance — and a tolerance
        // here would admit exactly the second statistic this test exists to
        // refuse.
        assert_eq!(
            alpha.deflated_excess.to_bits(),
            (deflated.observed - deflated.expected_maximum).to_bits(),
            "the family figure is not the gate's own deflation, to the bit"
        );

        // And it is demonstrably *not* the raw Sharpe the same series would
        // give. Twelve trials is a real search and the deflation is not zero.
        assert!(
            deflated.expected_maximum > 0.0,
            "the premise: twelve trials leave something to deflate, so the two figures differ"
        );
        let raw = members[0]
            .evidence
            .holdout
            .as_ref()
            .expect("the holdout series");
        let undeflated = stats::mean(&raw.holdout_returns) / stats::stddev(&raw.holdout_returns)
            * raw.periods_per_year.sqrt();
        assert!(
            (alpha.deflated_excess - undeflated).abs() > f64::EPSILON,
            "the family figure equals the undeflated Sharpe; the deflation has been dropped"
        );
    }

    #[test]
    fn a_family_s_count_is_the_whole_family_s_search_and_not_one_member_s_snapshot() {
        // The relation the one-member test above cannot see, and the claim
        // the module doc made falsely until 2026-09-14: the review deflates
        // every member against the family's lifetime total *as the review
        // finds it*, which is at least as large as the snapshot the gate
        // charged that member under and strictly larger for every member but
        // the last. That is deliberate — a comparison drawn now must correct
        // each family for the search it has actually done — and it is
        // asserted here so that a future edit "fixing" the review to use the
        // per-member snapshot fails rather than quietly re-introducing
        // selection bias on the review's own axis.
        let population = Population::new().enrol("alpha", 2, GateStage::Holdout, AHEAD);
        let members = population.members();
        assert_eq!(members.len(), 2, "the premise: two members were enrolled");
        let standings = standings(&members);
        let alpha = standings.get("alpha").expect("alpha stands");
        assert_eq!(
            alpha.admitted, 2,
            "the premise: both members' evidence was readable, so the mean is over two"
        );

        // The two snapshots, from the gate itself. Charged in enrolment
        // order, twelve trials each, so the first member's account froze at
        // twelve and the second's at twenty-four.
        let first = HoldoutGate::default()
            .deflated(members[0].strategy, members[0].evidence)
            .expect("the gate reads the first member");
        let second = HoldoutGate::default()
            .deflated(members[1].strategy, members[1].evidence)
            .expect("the gate reads the second member");
        assert!(
            second.trials > first.trials,
            "the premise: the two members were charged at different points in the family's \
             search, so their snapshots differ — {} against {}",
            first.trials,
            second.trials
        );

        // One count for the family, not one per member.
        assert_eq!(
            members[0].lifetime_trials, members[1].lifetime_trials,
            "the review read a different count for each member; the deflation is against the \
             family's search and a family has one"
        );
        let family_total = usize::try_from(members[0].lifetime_trials).expect("a small count");

        // The relation, both halves. `>=` for every member, and equality
        // exactly at the member charged last.
        for member in &members {
            let snapshot = HoldoutGate::default()
                .deflated(member.strategy, member.evidence)
                .expect("the gate reads this member");
            assert!(
                family_total >= snapshot.trials,
                "the family's current total {family_total} is below {}'s snapshot {}; a total \
                 that shrank means a charge was lost",
                member.strategy,
                snapshot.trials
            );
        }
        assert_eq!(
            family_total, second.trials,
            "equality falls on the member charged last, and the second member is it"
        );
        assert!(
            family_total > first.trials,
            "the first member's snapshot {} equals the family total {family_total}; the review \
             has stopped correcting for the siblings evaluated after it",
            first.trials
        );

        // And the figure follows from that count rather than from the
        // snapshots. Summed in the order `standings` sums, so this is bit
        // equality and not a tolerance.
        let mut total = 0.0_f64;
        for member in &members {
            let holdout = member
                .evidence
                .holdout
                .as_ref()
                .expect("the holdout series");
            let deflated = deflated_sharpe(
                &holdout.holdout_returns,
                family_total,
                holdout.periods_per_year,
            )
            .expect("the series deflates against the family total");
            total += deflated.observed - deflated.expected_maximum;
        }
        let at_family_total = total / 2.0;
        assert_eq!(
            alpha.deflated_excess.to_bits(),
            at_family_total.to_bits(),
            "the family figure is not the deflation at the family's own lifetime count"
        );
        let at_snapshots = ((first.observed - first.expected_maximum)
            + (second.observed - second.expected_maximum))
            / 2.0;
        assert!(
            (alpha.deflated_excess - at_snapshots).abs() > f64::EPSILON,
            "the family figure equals the mean of the per-member snapshots; the review is \
             grading each member against the search as it stood when that member ran, which \
             lets a family that has since tried ten thousand more configurations keep an old, \
             flattering correction"
        );
    }

    #[test]
    fn a_family_registered_ten_times_and_read_once_has_one_observation_and_not_ten() {
        // The bar is a *sample* bar — `COUNTERFACTUAL_SIZING_MIN_SAMPLE`, by
        // reference — and it was being applied to a population count until
        // 2026-09-14. A reviewer's probe built exactly this population and
        // got a finding reporting ten as the unfunded side's member count
        // from one readable observation, which is the number a reader takes
        // as the sample size.
        //
        // Both halves. The refusal, and then the same family with the
        // readable members it actually needs, so this cannot pass against a
        // `misallocation` that returns `None` for everything.
        let thin = Population::new()
            .enrol("alpha", 1, GateStage::Holdout, AHEAD)
            .enrol_unreadable("alpha", FAMILY_REVIEW_MIN_MEMBERS - 1, GateStage::Holdout)
            .enrol(
                "omega",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Scaled,
                BEHIND,
            );
        let thin_members = thin.members();
        let thin_standings = standings(&thin_members);
        let alpha = thin_standings.get("alpha").expect("alpha stands");
        assert_eq!(
            alpha.members, FAMILY_REVIEW_MIN_MEMBERS,
            "the premise: the family clears the bar on registrations"
        );
        assert_eq!(alpha.admitted, 1, "the premise: exactly one was readable");
        assert_eq!(alpha.refused, FAMILY_REVIEW_MIN_MEMBERS - 1);
        assert!(
            alpha.deflated_excess
                - thin_standings
                    .get("omega")
                    .expect("omega stands")
                    .deflated_excess
                >= FAMILY_REVIEW_MARGIN,
            "the premise: the one observation would clear the margin, so only the sample bar \
             can be what refuses this"
        );
        assert_eq!(
            misallocation(&thin_standings),
            None,
            "one readable member cleared a ten-observation bar because nine registrations \
             nothing could read were counted as observations"
        );

        // The admitting half: the same family, read ten times — and carrying
        // three further registrations nothing can read, so that the two
        // counts on the record differ and the field is proved to carry the
        // sample rather than the population.
        let full = Population::new()
            .enrol(
                "alpha",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Holdout,
                AHEAD,
            )
            .enrol_unreadable("alpha", 3, GateStage::Holdout)
            .enrol(
                "omega",
                FAMILY_REVIEW_MIN_MEMBERS,
                GateStage::Scaled,
                BEHIND,
            );
        let full_members = full.members();
        let full_standings = standings(&full_members);
        let full_alpha = full_standings.get("alpha").expect("alpha stands");
        assert_eq!(
            full_alpha.members,
            FAMILY_REVIEW_MIN_MEMBERS + 3,
            "the premise: the two counts differ, so the assertion below distinguishes them"
        );
        assert_eq!(full_alpha.admitted, FAMILY_REVIEW_MIN_MEMBERS);
        let finding = misallocation(&full_standings).expect("ten readable members is a finding");
        // And the record says how many observations it rests on, not how
        // many registrations exist.
        assert_eq!(
            finding.unfunded_evaluated, FAMILY_REVIEW_MIN_MEMBERS,
            "the finding reported the registration count as its sample size"
        );
        assert_eq!(finding.funded_evaluated, FAMILY_REVIEW_MIN_MEMBERS);
    }

    #[test]
    fn a_gap_of_exactly_the_margin_is_a_finding_and_one_ulp_below_it_is_not() {
        // `misallocation` argues its `>=` in a comment — "the margin is the
        // bar, and a gap that lands exactly on a declared threshold clears
        // it" — and nothing tested it: changing the operator to `>` left the
        // module's whole suite green. An argued bar with no test is an
        // argument.
        //
        // The standings are built directly rather than from evidence,
        // because a gap of *exactly* the margin cannot be reached by choosing
        // drifts. The margin less zero is exact in binary and `next_down` is
        // the nearest representable value below it, so the two cases below
        // are one unit in the last place apart and neither is a rounding
        // accident.
        let standing = |family: &str, funded: usize, excess: f64| FamilyStanding {
            family: family.to_string(),
            members: FAMILY_REVIEW_MIN_MEMBERS,
            funded,
            admitted: FAMILY_REVIEW_MIN_MEMBERS,
            refused: 0,
            deflated_excess: excess,
        };
        let pair = |unfunded_excess: f64| {
            BTreeMap::from([
                ("alpha".to_string(), standing("alpha", 0, unfunded_excess)),
                ("omega".to_string(), standing("omega", 1, 0.0)),
            ])
        };

        let exactly = pair(FAMILY_REVIEW_MARGIN);
        // Compared as bits: this premise is "exactly", and a float equality
        // here would be the one place in the test where "exactly" meant
        // something looser than the word.
        assert_eq!(
            (exactly.get("alpha").expect("alpha stands").deflated_excess
                - exactly.get("omega").expect("omega stands").deflated_excess)
                .to_bits(),
            FAMILY_REVIEW_MARGIN.to_bits(),
            "the premise: the gap is the margin exactly, to the bit"
        );
        let finding = misallocation(&exactly)
            .expect("a gap of exactly the declared margin did not clear the declared margin");
        assert_eq!(finding.unfunded, "alpha");

        let below = pair(FAMILY_REVIEW_MARGIN.next_down());
        assert!(
            below.get("alpha").expect("alpha stands").deflated_excess < FAMILY_REVIEW_MARGIN,
            "the premise: the second case is genuinely below the margin"
        );
        assert_eq!(
            misallocation(&below),
            None,
            "a gap one unit in the last place below the margin was read as clearing it"
        );
    }

    #[test]
    fn two_families_whose_names_differ_only_after_a_colon_cannot_exist_so_the_finding_key_is_injective()
     {
        // The finding's idempotency key joins two family names with `:`, so
        // its injectivity rests entirely on no family name containing one.
        // That is not a convention this module maintains — `StrategyFamily`
        // refuses the character, and says in its own refusal that the reason
        // is the journal key.
        let error = StrategyFamily::new("a:b").expect_err("a colon in a family name is refused");
        assert!(
            error.message().contains("journal key"),
            "the refusal no longer names the reason this key is safe: {}",
            error.message()
        );

        // The admitting half: two names that differ only where a colon could
        // have hidden the difference produce two distinct keys.
        let left = Misallocation {
            unfunded: StrategyFamily::new("a")
                .expect("admissible")
                .as_str()
                .to_string(),
            funded: StrategyFamily::new("b.c")
                .expect("admissible")
                .as_str()
                .to_string(),
            unfunded_evaluated: FAMILY_REVIEW_MIN_MEMBERS,
            funded_evaluated: FAMILY_REVIEW_MIN_MEMBERS,
        };
        let right = Misallocation {
            unfunded: StrategyFamily::new("a.b")
                .expect("admissible")
                .as_str()
                .to_string(),
            funded: StrategyFamily::new("c")
                .expect("admissible")
                .as_str()
                .to_string(),
            ..left.clone()
        };
        let key = |pair: &Misallocation| {
            MisallocationFinding::of(pair, FAMILY_FINDING_PROPOSED, 3, at())
                .idempotency_key()
                .expect("the finding carries a key")
        };
        assert_ne!(key(&left), key(&right));
        // And the two bodies on this one topic do not collide either.
        assert_ne!(
            key(&left),
            FamilyAllocationReview::of(&BTreeMap::new(), 3, at())
                .idempotency_key()
                .expect("the review carries a key")
        );
    }

    #[test]
    fn the_review_orders_families_by_name_so_two_replays_of_one_log_produce_one_journal_line() {
        // Three unfunded families carrying byte-identical evidence, so the
        // leader is decided purely by iteration order, plus one funded family
        // well behind. Under a map with no order the pair named would vary
        // between replays and the log would hold two records for one fact.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let population = Population::new()
                .enrol(
                    "charlie",
                    FAMILY_REVIEW_MIN_MEMBERS,
                    GateStage::Holdout,
                    AHEAD,
                )
                .enrol(
                    "alpha",
                    FAMILY_REVIEW_MIN_MEMBERS,
                    GateStage::Holdout,
                    AHEAD,
                )
                .enrol(
                    "bravo",
                    FAMILY_REVIEW_MIN_MEMBERS,
                    GateStage::Holdout,
                    AHEAD,
                )
                .enrol(
                    "omega",
                    FAMILY_REVIEW_MIN_MEMBERS,
                    GateStage::Scaled,
                    BEHIND,
                );
            let members = population.members();
            let standings = standings(&members);
            let names: Vec<&str> = standings
                .values()
                .map(|standing| standing.family.as_str())
                .collect();
            assert_eq!(
                names,
                vec!["alpha", "bravo", "charlie", "omega"],
                "the standings are not in name order"
            );
            let review = FamilyAllocationReview::of(&standings, 11, at());
            let finding = misallocation(&standings).expect("the premise: a finding is produced");
            assert_eq!(
                finding.unfunded_evaluated, FAMILY_REVIEW_MIN_MEMBERS,
                "the premise: three families tied on evidence, so only the order decides"
            );
            seen.insert(format!(
                "{}|{}",
                serde_json::to_string(&review).expect("the review serialises"),
                MisallocationFinding::of(&finding, FAMILY_FINDING_PROPOSED, 11, at())
                    .idempotency_key()
                    .expect("the finding carries a key")
            ));
        }
        assert_eq!(
            seen.len(),
            1,
            "sixty-four identical reviews produced {} distinct records: {seen:?}",
            seen.len()
        );
    }
}
