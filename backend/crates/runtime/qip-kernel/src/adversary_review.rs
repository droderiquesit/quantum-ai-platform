//! Blueprint §15.2's adversary monitor: who filled the platform's orders, and
//! whether that venue's behaviour toward it is changing.
//!
//! §15.2 opens by saying version 8.0 estimated adverse selection — the
//! symptom — and never modelled who caused it or how they adapt. It then asks
//! five questions and names four responses. **This module answers one of the
//! five and produces none of the four**, and the honesty of that sentence is
//! the point of the rest of this comment: the other four questions need data
//! nothing in this platform ingests, and three of the four responses need a
//! seam this platform does not have. What is built is built from a
//! measurement the platform already makes, and what is not built is named
//! below rather than approximated.
//!
//! # The one question this can answer, and what makes it answerable
//!
//! *"Is a counterparty adapting to me? Deteriorating fill quality on a
//! specific venue against a stable market is a signal about a counterparty,
//! not the market."*
//!
//! The platform already holds, per fill, the difference between what the
//! venue charged and what the counterfactual twin says the *same order* would
//! have cost on the *same tape* under the *same cost model*:
//! [`FillScore::trade_error_bps`], signed so that a negative value means the
//! twin filled better than reality. Negate it and you have a **shortfall** —
//! how much worse than modelled the platform was actually filled, in basis
//! points. The twin is the "stable market" control §15.2 asks for: it reprices
//! against the tape, so the market component is held fixed by construction and
//! what is left over is about the venue.
//!
//! # Why the level is reported and only the drift is a finding
//!
//! A venue's *level* of shortfall is confounded and this module says so in the
//! type: the twin's cost model ([`qip_simulation_engine::costs::CostModel`]'s
//! liquid-equity profile) is an estimate, and a model that is systematically
//! too generous reads as every venue filling badly. That is a mis-specified
//! model, not an adversary. So [`AdversaryPosture::Deteriorating`] is a
//! posture and never a finding, and its documentation names the confound.
//!
//! The *drift* is not confounded in the same way. The cost model is constant
//! across the window, so it cancels in the difference between a venue's early
//! half and its recent half. A venue whose recent fills are materially worse
//! than its own earlier fills, on a tape the twin priced both halves against,
//! has changed its behaviour toward this platform. That is
//! [`AdversaryPosture::Adapting`], and it is the only arm this module will
//! call a finding.
//!
//! # What this deliberately does not do
//!
//! **It withdraws nothing.** ADR 0062's guarantee is that a venue is
//! withdrawn on feasibility evidence and on nothing else, and
//! `qip-kernel/tests/learning.rs::a_twin_that_is_wildly_wrong_about_fills_never_withdraws_a_venue`
//! holds that line by mispricing twelve fills by a thousand basis points and
//! asserting no venue moves. A monitor that could withdraw on its own
//! evidence would make a mis-specified cost model able to stop the platform
//! trading. The finding is a record and a shipped profile; the consequence,
//! if the desk ever wants one, is a separate decision with a separate ADR.
//!
//! **It does not classify flow.** "Who is on the other side of my fills"
//! needs a trade tape carrying an aggressor side or a counterparty, and the
//! platform ingests bars, macro releases and alternative-data readings
//! (`qip_market_ingestion::tape`). The only execution target is the simulated
//! broker (`.claude/rules/domains/risk-and-execution.md`), which has no other
//! side at all. A classifier fed bar volume would be a label nobody measured.
//!
//! **It does not measure crowding.** "Correlated flow arriving simultaneously
//! with the platform's own" requires the platform's own flow to be visible in
//! the same tape as everyone else's. Against a simulated broker the
//! platform's orders never enter the tape, so the correlation would be
//! between a series and a series that is definitionally absent from it.
//!
//! **It does not randomise a fingerprint.** Jitter, randomised child sizing
//! and venue rotation exist in §15.2 to defeat a counterparty's detection of
//! this platform. This platform is paper-trading and submits no live order,
//! so there is no counterparty to defeat; building the capability would be
//! building, against the day the boundary moved, a thing whose only purpose
//! is to evade another participant's controls. It is not built, and this
//! paragraph is the record of the refusal rather than a backlog item.
//!
//! **It does not widen a quote.** Toxic-flow widening presumes the platform
//! posts two-sided prices whose spread it sets. The desk sends orders; the
//! cells rest them. There is no spread of the platform's own to widen, so a
//! `widening_bps` field on a profile would be a number with no reader — the
//! `MaxExpectedShortfall` failure in a new place.
//!
//! # No money crosses this module
//!
//! Every figure here is a basis-point ratio the twin already reduced to
//! `f64` at `platform::fill_error_bps`, which is where the one `Decimal →
//! f64` crossing on this path happens and is documented. Nothing in this
//! file is money, nothing here can be booked, and a non-finite reading is
//! treated as **unmeasured** rather than as zero: an error nobody could
//! compute is not an error of nothing.

use crate::platform::{
    COUNTERFACTUAL_SIZING_MIN_SAMPLE, COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION, FillScore,
    Platform,
};
use qip_contracts::policy::{AdversaryProfiles, Slot};
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The producer every record this module writes carries.
///
/// It is also the filter [`last_postures`] reads back on. [`Topic::LessonRecorded`]
/// is a general Learn topic with no other body on it today, and a second body
/// arriving on it later must not be decoded as this one — filtering on the
/// producer is what makes that safe, and it is the discipline
/// `Platform::records_on` already uses for the two venue readers.
pub const ADVERSARY_REVIEW_ORIGIN: &str = "kernel/adversary-review";

/// How many recent fill scores the monitor judges a venue over.
///
/// A rate window, the [`crate::venue_review::FEASIBILITY_WINDOW`] discipline:
/// the question is "how is this venue filling me *lately*", so the oldest
/// leaving is the sample staying current. Stated here rather than inherited
/// from the producer's own bound on `Platform::fill_scores`, because a change
/// to that bound must not silently change what "lately" means — and because
/// an unstated dependency on somebody else's cap is how a bounded working set
/// stops being bounded.
pub const ADVERSARY_WINDOW: usize = 256;

/// Measured fills at one venue before any posture above
/// [`AdversaryPosture::Unmeasured`] is reachable, and — applied to each half
/// separately — before a drift can be computed.
///
/// Ten, by reference to [`COUNTERFACTUAL_SIZING_MIN_SAMPLE`], which is
/// `qip_learning_engine::self_model::MINIMUM_SAMPLE`. ADR 0055 argues why ten
/// observations is where a pattern stops being noise and that argument is not
/// restated with a second, differently-sized number: the platform has one
/// answer to "how much evidence makes a pattern a finding".
///
/// Applying it *per half* means [`AdversaryPosture::Adapting`] needs twenty
/// measured fills at one venue. That is deliberate and it is the expensive
/// half of the bar: a drift is a comparison of two estimates, and an estimate
/// from five fills compared with an estimate from five fills is two noises
/// subtracted.
pub const ADVERSARY_MIN_SAMPLE: usize = COUNTERFACTUAL_SIZING_MIN_SAMPLE;

/// The share of a venue's measured fills that must be materially adverse
/// before its posture reads [`AdversaryPosture::Deteriorating`] — three in
/// four, by reference to [`COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION`], for
/// the reason above.
pub const ADVERSARY_ADVERSE_FRACTION: f64 = COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION;

/// The basis-point shortfall at which a fill is "materially" worse than the
/// twin modelled, and the basis-point deterioration at which a venue's drift
/// becomes a finding. One number for both, because both ask the same
/// question — is this bigger than the modelling error it would otherwise be
/// attributed to.
///
/// Five, anchored rather than chosen: the twin prices a desk fill under
/// `CostModel::liquid_equity()`, whose entry cost before impact is a 2.5 bp
/// half-spread plus 1.0 bp commission
/// (`qip_financial::costs::TransactionCostModel::default`). Five sits clear of
/// that whole modelled cost, so a fill counted adverse here is one the venue
/// charged more for than the twin's *entire* modelled cost of entering a
/// liquid name — not one where the model was a little off.
///
/// `the_material_bar_sits_clear_of_the_twins_own_modelled_entry_cost` asserts
/// the relationship rather than the number, so widening the default cost model
/// past this bar fails a test instead of quietly turning every venue adverse.
pub const ADVERSARY_MATERIAL_BPS: f64 = 5.0;

/// What the monitor has concluded about one venue.
///
/// Ordered worst-last so a caller comparing two postures can say which is the
/// stronger claim without a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdversaryPosture {
    /// Fewer than [`ADVERSARY_MIN_SAMPLE`] fills at this venue carried a
    /// comparable twin price. Not "fine" — *unknown*, and the distinction is
    /// the whole reason this arm exists rather than defaulting to
    /// [`Self::Benign`].
    Unmeasured,
    /// Enough evidence to speak, and nothing in it: neither the level nor the
    /// drift clears its bar.
    Benign,
    /// At least [`ADVERSARY_ADVERSE_FRACTION`] of this venue's measured fills
    /// were materially worse than the twin modelled.
    ///
    /// **A posture and never a finding.** The twin's cost model is an
    /// estimate; one that is systematically too generous puts every venue
    /// here and says nothing about any counterparty. Read it as "the desk's
    /// cost assumptions and this venue's prices disagree", which is worth
    /// knowing and is not evidence of an adversary.
    Deteriorating,
    /// This venue's recent half is worse than its own early half by at least
    /// [`ADVERSARY_MATERIAL_BPS`], on at least [`ADVERSARY_MIN_SAMPLE`]
    /// measured fills in each half.
    ///
    /// The one arm this module calls a finding, because the cost model is
    /// constant across the window and cancels in the difference. §15.2's
    /// third question, answered.
    Adapting,
}

impl AdversaryPosture {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unmeasured => "unmeasured",
            Self::Benign => "benign",
            Self::Deteriorating => "deteriorating",
            Self::Adapting => "adapting",
        }
    }
}

/// One venue's adversary profile over the window.
///
/// Carries the arithmetic a reader needs to check the posture rather than
/// only the posture, the [`crate::venue_review::VenueCluster`] discipline: a
/// conclusion whose inputs are not on the record is one nobody can dispute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueAdversaryProfile {
    pub venue: String,
    /// Fills at this venue in the window, measured or not — the honest
    /// denominator.
    pub sample: usize,
    /// Fills whose twin price could be compared. `sample - measured` is the
    /// number the twin declined to price, which is not the same as a venue
    /// filling well.
    pub measured: usize,
    /// Measured fills whose shortfall reached [`ADVERSARY_MATERIAL_BPS`].
    pub adverse: usize,
    /// `adverse` over `measured`, or zero where nothing was measured.
    pub adverse_share: f64,
    /// Mean shortfall over the older half of the measured fills, in basis
    /// points, present only when that half reaches [`ADVERSARY_MIN_SAMPLE`].
    pub early_mean_shortfall_bps: Option<f64>,
    /// The same over the newer half.
    pub recent_mean_shortfall_bps: Option<f64>,
    /// `recent - early`. Positive means the venue is filling this platform
    /// worse than it used to.
    pub drift_bps: Option<f64>,
    pub posture: AdversaryPosture,
}

impl VenueAdversaryProfile {
    /// One line an operator can read without opening the record.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "{}: {} ({} of {} fill(s) measured, {} materially adverse)",
            self.venue,
            self.posture.as_str(),
            self.measured,
            self.sample,
            self.adverse
        );
        if let Some(drift) = self.drift_bps {
            line.push_str(&format!(", drift {drift:+.1} bps"));
        }
        line
    }
}

/// Every venue's profile over the most recent [`ADVERSARY_WINDOW`] fill
/// scores, keyed by venue.
///
/// Pure. A `BTreeMap` because the result is serialised into a signed policy
/// slot and journaled: a map that iterated in two orders would sign as two
/// payloads and replay as two records.
///
/// The window is the **tail** of `scores`, which `Platform::score_filled`
/// pushes oldest-first, so "early" and "recent" below mean what they say.
pub fn assess(scores: &[FillScore]) -> BTreeMap<String, VenueAdversaryProfile> {
    let window = &scores[scores.len().saturating_sub(ADVERSARY_WINDOW)..];
    // Venue → (total fills, the finite shortfalls in arrival order).
    let mut by_venue: BTreeMap<&str, (usize, Vec<f64>)> = BTreeMap::new();
    for score in window {
        let entry = by_venue
            .entry(score.venue.as_str())
            .or_insert((0, Vec::new()));
        entry.0 += 1;
        // Negate: `trade_error_bps` is signed so that negative means the twin
        // filled better than reality, and what this module reasons about is
        // how much *worse* than modelled the venue filled. A non-finite
        // reading is dropped to the unmeasured count rather than folded in as
        // zero — the refuse-don't-clamp rule, and also the only thing keeping
        // `serde_json` able to serialise the profile at all.
        if let Some(error) = score.trade_error_bps.filter(|bps| bps.is_finite()) {
            entry.1.push(-error);
        }
    }
    by_venue
        .into_iter()
        .map(|(venue, (sample, shortfalls))| {
            (venue.to_string(), profile(venue, sample, &shortfalls))
        })
        .collect()
}

/// One venue's arithmetic, split out so the branch structure is readable and
/// testable on its own.
fn profile(venue: &str, sample: usize, shortfalls: &[f64]) -> VenueAdversaryProfile {
    let measured = shortfalls.len();
    let adverse = shortfalls
        .iter()
        .filter(|bps| **bps >= ADVERSARY_MATERIAL_BPS)
        .count();
    // usize → f64: a ratio of counts, in the statistics lane, and it is a
    // ratio of things the twin measured rather than of money.
    let adverse_share = if measured == 0 {
        0.0
    } else {
        adverse as f64 / measured as f64
    };
    // The split point. The newer half takes the extra observation on an odd
    // count, because the newer half is the one under test.
    let mid = measured / 2;
    let early = mean_of(&shortfalls[..mid]);
    let recent = mean_of(&shortfalls[mid..]);
    let drift = early.zip(recent).map(|(early, recent)| recent - early);
    let posture = if drift.is_some_and(|drift| drift >= ADVERSARY_MATERIAL_BPS) {
        AdversaryPosture::Adapting
    } else if measured < ADVERSARY_MIN_SAMPLE {
        AdversaryPosture::Unmeasured
    } else if adverse_share >= ADVERSARY_ADVERSE_FRACTION {
        AdversaryPosture::Deteriorating
    } else {
        AdversaryPosture::Benign
    };
    VenueAdversaryProfile {
        venue: venue.to_string(),
        sample,
        measured,
        adverse,
        adverse_share,
        early_mean_shortfall_bps: early,
        recent_mean_shortfall_bps: recent,
        drift_bps: drift,
        posture,
    }
}

/// The mean of a half, or `None` where the half is too small to estimate
/// anything from.
///
/// `None` rather than a mean over three observations: a drift computed from a
/// half nobody could estimate is a finding about arithmetic.
fn mean_of(half: &[f64]) -> Option<f64> {
    if half.len() < ADVERSARY_MIN_SAMPLE {
        return None;
    }
    // usize → f64: a count in the denominator of a statistic.
    let mean = half.iter().sum::<f64>() / half.len() as f64;
    mean.is_finite().then_some(mean)
}

/// Blueprint §41.5's twelfth slot, produced.
///
/// The slot's own type calls this "the adversary monitor's opaque summary"
/// and carries `BTreeMap<String, serde_json::Value>`; this is the monitor and
/// this is the summary. Until it existed the slot shipped
/// [`Slot::unproduced`] in every payload the centre has ever built, which
/// reads at a cell as `Freshness::Unavailable`.
///
/// # Two refusals, both fail-closed
///
/// A slot is produced only when at least one venue's posture is something
/// other than [`AdversaryPosture::Unmeasured`]. A profile set in which
/// nothing was measured is not a measurement, and shipping it would turn slot
/// 12 fresh on the strength of having run.
///
/// And if any venue's profile cannot be serialised, **nothing** is produced
/// rather than the rest. A partial set silently missing the one venue that
/// failed to encode is worse than no set: the venue that dropped out is
/// exactly the one a reader was looking for. (Reachable only through a
/// non-finite figure, which [`assess`] already excludes — belt and braces on
/// a path whose failure mode is a false reassurance.)
///
/// # What producing it can do at a cell
///
/// Nothing, today, and that is by design.
/// `qip_contracts::policy::PolicyItem::capability` maps this item to no §6.2
/// capability, so a fresh slot 12 changes no sizing multiplier and lifts no
/// pause; it changes the payload digest and it gives a cell something to read
/// if one ever reads it. Slot 12 cannot widen what a cell trades for the same
/// structural reason slot 11 cannot.
pub fn slot(scores: &[FillScore], now: Timestamp) -> Slot<AdversaryProfiles> {
    let profiles = assess(scores);
    if profiles
        .values()
        .all(|profile| profile.posture == AdversaryPosture::Unmeasured)
    {
        return Slot::unproduced();
    }
    let mut venues = BTreeMap::new();
    for (venue, profile) in profiles {
        match serde_json::to_value(&profile) {
            Ok(value) => {
                venues.insert(venue, value);
            }
            Err(_) => return Slot::unproduced(),
        }
    }
    Slot::produced(AdversaryProfiles { venues }, now)
}

/// A venue's posture changed — the record §15.2's third question produces.
///
/// On [`Topic::LessonRecorded`], which is a Learn-group topic retained for
/// audit and which no other body in this workspace fills. A lesson is exactly
/// what this is: it changes nothing the platform does, and it exists so that
/// the question "was this venue always filling us like this" has an answer
/// that is not somebody's memory.
///
/// Journaled **on change only**, the [`crate::sizing_review::SizingCapEntry`]
/// discipline. A venue that has been `benign` for a year writes one record,
/// not three hundred and sixty-five.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdversaryPostureChanged {
    pub venue: String,
    pub posture: AdversaryPosture,
    /// What the log last said about this venue. [`AdversaryPosture::Unmeasured`]
    /// where the log says nothing, which is what "never measured" means.
    pub previous: AdversaryPosture,
    pub sample: usize,
    pub measured: usize,
    pub adverse: usize,
    pub adverse_share: f64,
    pub early_mean_shortfall_bps: Option<f64>,
    pub recent_mean_shortfall_bps: Option<f64>,
    pub drift_bps: Option<f64>,
    pub cycle: u64,
    pub at: Timestamp,
}

impl AdversaryPostureChanged {
    pub fn of(
        profile: &VenueAdversaryProfile,
        previous: AdversaryPosture,
        cycle: u64,
        at: Timestamp,
    ) -> Self {
        Self {
            venue: profile.venue.clone(),
            posture: profile.posture,
            previous,
            sample: profile.sample,
            measured: profile.measured,
            adverse: profile.adverse,
            adverse_share: profile.adverse_share,
            early_mean_shortfall_bps: profile.early_mean_shortfall_bps,
            recent_mean_shortfall_bps: profile.recent_mean_shortfall_bps,
            drift_bps: profile.drift_bps,
            cycle,
            at,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "venue {} moved from {} to {} on {} of {} measured fill(s)",
            self.venue,
            self.previous.as_str(),
            self.posture.as_str(),
            self.adverse,
            self.measured
        )
    }
}

impl EventBody for AdversaryPostureChanged {
    const TOPIC: Topic = Topic::LessonRecorded;
    const SCHEMA_VERSION: u32 = 1;

    /// Keyed on the venue, the posture it moved *to*, and the cycle, so a
    /// review that journaled and then failed cannot write twice on retry
    /// within one cycle, and a venue that returns to a posture in a later
    /// cycle writes a second record rather than being silently swallowed by
    /// the first.
    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "adversary-posture:{}:{}:{}",
            self.venue,
            self.posture.as_str(),
            self.cycle
        ))
    }
}

/// The last posture the log holds for each venue, in log order.
///
/// Read from the event log rather than from a field on `Platform`, which is
/// the `Platform::venue_withdrawals` discipline: the log is the record, and a
/// second table would be a second claim about the same fact that could
/// disagree with it. It also means a restarted process resumes knowing what it
/// already said about each venue and does not re-journal the whole fleet.
///
/// Filtered by producer as well as topic, so a body some other lane later puts
/// on [`Topic::LessonRecorded`] cannot be decoded as this one.
pub fn last_postures(platform: &Platform) -> Result<BTreeMap<String, AdversaryPosture>> {
    let mut postures = BTreeMap::new();
    for event in platform
        .event_log()
        .by_topic(AdversaryPostureChanged::TOPIC)
        .into_iter()
        .filter(|event| event.lineage.producer == ADVERSARY_REVIEW_ORIGIN)
    {
        let body = StreamEnvelope::from_frame(event)?
            .decode::<AdversaryPostureChanged>()?
            .body;
        postures.insert(body.venue, body.posture);
    }
    Ok(postures)
}

/// The LEARN stage's adversary review: blueprint §15.2's third question,
/// measured and put on the record.
///
/// Two effects and neither moves anything. The profiles are measured from the
/// fill scores the twin wrote, and every venue whose posture differs from what
/// the log last said about it is journaled. There is no third effect: no
/// venue is withdrawn, no size narrowed, no bound moved, no quote widened —
/// see this module's header for why each of those is refused rather than
/// deferred.
///
/// Returns the `(summary, problems)` shape every other LEARN review returns,
/// so the stage folds it the same way. The summary is `Some` whenever any
/// venue was measured, **including when nothing changed**: a review that went
/// silent on a quiet cycle reads in a cycle report exactly like a review that
/// never ran, and those mean opposite things. It is `None` only when the
/// monitor genuinely has nothing — no fill has been scored at any venue.
///
/// A log this cannot read is a problem on the cycle and stops the journaling
/// outright rather than proceeding against an empty prior: a review that
/// believed the log said nothing would re-journal every venue it knows, every
/// cycle, for as long as the log stayed unreadable.
pub fn review(platform: &mut Platform, now: Timestamp) -> (Option<String>, Vec<String>) {
    let profiles = assess(platform.fill_scores());
    if profiles.is_empty() {
        return (None, Vec::new());
    }
    let cycle = platform.cycle_count();
    let mut problems = Vec::new();
    let mut parts: Vec<String> = profiles
        .values()
        .map(VenueAdversaryProfile::describe)
        .collect();
    let prior = match last_postures(platform) {
        Ok(prior) => prior,
        Err(error) => {
            problems.push(format!(
                "the adversary review could not read what the log already says about each venue, \
                 so no posture change was journaled this cycle: {}",
                error.message()
            ));
            return (Some(parts.join("; ")), problems);
        }
    };
    let changed: Vec<AdversaryPostureChanged> = profiles
        .values()
        .filter(|profile| profile.posture != AdversaryPosture::Unmeasured)
        .filter_map(|profile| {
            let previous = prior
                .get(&profile.venue)
                .copied()
                .unwrap_or(AdversaryPosture::Unmeasured);
            (previous != profile.posture)
                .then(|| AdversaryPostureChanged::of(profile, previous, cycle, now))
        })
        .collect();
    for record in changed {
        let described = record.describe();
        match platform.journal_once(record, ADVERSARY_REVIEW_ORIGIN, now) {
            Ok(true) => parts.push(described),
            // Already on the record for this cycle — the idempotency key did
            // its job on a retry, which is not a problem and not a change.
            Ok(false) => {}
            Err(error) => problems.push(format!(
                "an adversary posture change could not be journaled ({described}): {}",
                error.message()
            )),
        }
    }
    (Some(parts.join("; ")), problems)
}

/// Every posture change this kernel has journaled, oldest first.
///
/// Read off the log for the reason `Platform::venue_withdrawals` gives: the
/// log is the record, and the postures [`last_postures`] resumes from are
/// exactly these events, so a reader comparing the two is comparing a
/// derivation with its source rather than two independent claims.
pub fn posture_changes(platform: &Platform) -> Result<Vec<AdversaryPostureChanged>> {
    platform
        .event_log()
        .by_topic(AdversaryPostureChanged::TOPIC)
        .into_iter()
        .filter(|event| event.lineage.producer == ADVERSARY_REVIEW_ORIGIN)
        .map(|event| {
            Ok(StreamEnvelope::from_frame(event)?
                .decode::<AdversaryPostureChanged>()?
                .body)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::policy::PolicyItem;
    use qip_core::ids::{ObjectId, OrderId};
    use qip_financial::costs::TransactionCostModel;

    fn score(venue: &str, error_bps: Option<f64>, n: usize) -> FillScore {
        FillScore {
            order_id: OrderId::from_string(format!("order-{venue}-{n}")),
            object_id: ObjectId::from_string(format!("obj-{n}")),
            venue: venue.to_string(),
            filled_at: Timestamp::from_secs(1_000 + n as i64),
            scored_at: Timestamp::from_secs(2_000 + n as i64),
            smaller_favoured: false,
            larger_favoured: false,
            trade_error_bps: error_bps,
        }
    }

    /// `count` fills at `venue`, each with the same twin error.
    fn run(venue: &str, count: usize, error_bps: f64, from: usize) -> Vec<FillScore> {
        (from..from + count)
            .map(|n| score(venue, Some(error_bps), n))
            .collect()
    }

    /// A platform with an empty universe — enough to hold an event log, which
    /// is all the two log tests below need.
    fn bare_platform() -> Platform {
        let config = crate::config::PlatformConfig::default();
        let (context, _clock) =
            qip_core::Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
        Platform::new(
            config,
            context,
            qip_observability::Telemetry::silent(),
            qip_financial::universe::Universe::new(),
            qip_risk::limits::LimitSet::new("adversary-unit"),
        )
        .expect("a platform with an empty universe")
    }

    #[test]
    fn a_posture_record_written_by_another_producer_is_not_read_back_as_this_monitors_word() {
        // `Topic::LessonRecorded` is a general Learn topic and this module is
        // the only body on it *today*. A second producer putting a record
        // there later must not become what `last_postures` thinks the monitor
        // last said — because the monitor would then journal nothing, and the
        // finding it had reached would never reach the log at all. The filter
        // is on the producer, and this is the input that proves it fires: the
        // same body, the same topic, a different origin.
        let mut platform = bare_platform();
        let now = Timestamp::from_secs(1_760_000_000);
        let foreign = AdversaryPostureChanged::of(
            &profile("v", 20, &[10.0; 20]),
            AdversaryPosture::Benign,
            1,
            now,
        );
        assert!(
            platform
                .journal_once(foreign, "kernel/some-other-lane", now)
                .expect("the log accepts the record"),
            "the premise failed: the foreign record was not written, so there is nothing to \
             filter out"
        );
        assert!(
            posture_changes(&platform)
                .expect("the log reads back")
                .is_empty(),
            "a record another producer wrote on this topic was read back as this monitor's"
        );
        assert!(
            last_postures(&platform)
                .expect("the log reads back")
                .is_empty(),
            "a record another producer wrote on this topic became the monitor's prior"
        );
    }

    #[test]
    fn a_posture_this_monitor_wrote_is_read_back_as_the_prior_for_that_venue() {
        // The other half, and the reason the test above is not vacuous: a
        // filter that excluded everything would satisfy it for ever. The
        // monitor's own record must come back, keyed by its venue.
        let mut platform = bare_platform();
        let now = Timestamp::from_secs(1_760_000_000);
        let own = AdversaryPostureChanged::of(
            &profile("v", 20, &[10.0; 20]),
            AdversaryPosture::Benign,
            1,
            now,
        );
        assert!(
            platform
                .journal_once(own, ADVERSARY_REVIEW_ORIGIN, now)
                .expect("the log accepts the record"),
            "the premise failed: the monitor's own record was not written"
        );
        let prior = last_postures(&platform).expect("the log reads back");
        assert_eq!(prior.get("v"), Some(&AdversaryPosture::Deteriorating));
        assert_eq!(
            posture_changes(&platform)
                .expect("the log reads back")
                .len(),
            1
        );
    }

    #[test]
    fn the_material_bar_sits_clear_of_the_twins_own_modelled_entry_cost() {
        // The reason `ADVERSARY_MATERIAL_BPS` is five and not a number
        // somebody liked. The twin prices a desk fill under
        // `CostModel::liquid_equity()`, which is built from
        // `TransactionCostModel::default()`; if that model's entry cost ever
        // grows past this bar, every venue reads adverse on the cost model
        // alone and `Deteriorating` becomes a posture that fires on nothing
        // at all. Asserting the relationship rather than the numbers is what
        // makes this fail when the cost model moves, which a test comparing
        // 5.0 with 5.0 would not.
        let model = TransactionCostModel::default();
        let modelled_entry_bps = model.half_spread_bps + model.commission_bps;
        assert!(
            modelled_entry_bps > 0.0,
            "the premise failed: the default cost model charges nothing to enter, so there is no \
             bar to sit clear of"
        );
        assert!(
            ADVERSARY_MATERIAL_BPS > modelled_entry_bps,
            "the material bar is {ADVERSARY_MATERIAL_BPS} bps and the twin's own modelled entry \
             cost is {modelled_entry_bps} bps; a fill counted adverse is now one the cost model \
             alone explains"
        );
    }

    #[test]
    fn a_venue_with_too_few_measured_fills_is_unmeasured_and_never_benign() {
        // The distinction the `Unmeasured` arm exists for. Nine fills, every
        // one of them filled better than modelled — as benign as evidence
        // gets — and the monitor still refuses to call it benign, because
        // nine is below the bar. A monitor that defaulted to "benign" would
        // report every venue it has never looked at as clean.
        let scores = run("v", ADVERSARY_MIN_SAMPLE - 1, -20.0, 0);
        assert_eq!(
            scores.len(),
            9,
            "the premise failed: the fixture is not one short of the bar"
        );
        let profiles = assess(&scores);
        let profile = profiles.get("v").expect("the venue is profiled");
        assert_eq!(profile.measured, 9);
        assert_eq!(profile.posture, AdversaryPosture::Unmeasured);
    }

    #[test]
    fn a_venue_filling_materially_worse_than_modelled_reads_deteriorating_and_not_adapting() {
        // Ten fills, every one twenty basis points worse than the twin
        // modelled, and no second half to compare against — the level clears
        // its bar and the drift cannot be computed, so the posture stops at
        // the confounded arm. The assertion that it is *not* `Adapting` is
        // the load-bearing one: a level that could promote itself to a
        // finding would let a mis-specified cost model accuse a venue.
        let scores = run("v", ADVERSARY_MIN_SAMPLE, -20.0, 0);
        let profiles = assess(&scores);
        let profile = profiles.get("v").expect("the venue is profiled");
        assert_eq!(
            profile.adverse, ADVERSARY_MIN_SAMPLE,
            "the premise failed: the fixture's fills are not materially adverse"
        );
        assert!(
            profile.drift_bps.is_none(),
            "the premise failed: ten fills produced two halves of ten"
        );
        assert_eq!(profile.posture, AdversaryPosture::Deteriorating);
    }

    #[test]
    fn a_venue_whose_recent_half_is_materially_worse_than_its_early_half_reads_adapting() {
        // §15.2's third question, firing. Ten fills the twin priced almost
        // exactly, then ten filled six basis points worse — a drift of six,
        // clear of the five-point bar — on a venue whose *level* never
        // reaches the deteriorating share. So the finding cannot be coming
        // from the level, which is the point of the fixture.
        let mut scores = run("v", ADVERSARY_MIN_SAMPLE, 0.0, 0);
        scores.extend(run("v", ADVERSARY_MIN_SAMPLE, -6.0, 100));
        let profiles = assess(&scores);
        let profile = profiles.get("v").expect("the venue is profiled");
        assert_eq!(
            profile.measured,
            2 * ADVERSARY_MIN_SAMPLE,
            "the premise failed: not every fill was measured"
        );
        let drift = profile.drift_bps.expect("two full halves produce a drift");
        assert!(
            (drift - 6.0).abs() < 1e-9,
            "the drift reads {drift} where the fixture moved six basis points"
        );
        assert!(
            profile.adverse_share < ADVERSARY_ADVERSE_FRACTION,
            "the premise failed: the level alone would have produced a posture, so this test \
             cannot tell the drift from the level ({} of {})",
            profile.adverse,
            profile.measured
        );
        assert_eq!(profile.posture, AdversaryPosture::Adapting);
    }

    #[test]
    fn a_venue_whose_recent_half_is_better_than_its_early_half_never_reads_adapting() {
        // The sign. A venue that has *improved* by six basis points has the
        // same magnitude of drift as one that deteriorated by six, and a
        // detector written against `abs()` would call both adapting —
        // reporting a venue that started treating the platform better as an
        // adversary. This fixture is the earlier one reversed.
        let mut scores = run("v", ADVERSARY_MIN_SAMPLE, -6.0, 0);
        scores.extend(run("v", ADVERSARY_MIN_SAMPLE, 0.0, 100));
        let profiles = assess(&scores);
        let profile = profiles.get("v").expect("the venue is profiled");
        let drift = profile.drift_bps.expect("two full halves produce a drift");
        assert!(
            (drift + 6.0).abs() < 1e-9,
            "the premise failed: the fixture did not improve by six basis points, drift is {drift}"
        );
        assert_ne!(profile.posture, AdversaryPosture::Adapting);
    }

    #[test]
    fn a_fill_the_twin_could_not_price_is_counted_unmeasured_and_never_as_no_error() {
        // A `None` error is the twin declining to price, and folding it in as
        // zero would make a venue whose fills the twin cannot price at all
        // read as a venue filling exactly as modelled — the most reassuring
        // possible answer produced from no evidence. Twenty unpriceable fills
        // and one venue: the sample is twenty, the measurement is nothing.
        let scores: Vec<FillScore> = (0..20).map(|n| score("v", None, n)).collect();
        let profiles = assess(&scores);
        let profile = profiles.get("v").expect("the venue is profiled");
        assert_eq!(profile.sample, 20);
        assert_eq!(profile.measured, 0);
        assert_eq!(profile.drift_bps, None);
        assert_eq!(profile.posture, AdversaryPosture::Unmeasured);
    }

    #[test]
    fn a_non_finite_twin_error_is_dropped_rather_than_carried_into_the_slot() {
        // `serde_json` refuses to serialise a non-finite float, so a NaN that
        // reached a profile would make `slot` produce nothing at all and the
        // centre would ship an unproduced slot with no explanation. It is
        // dropped to the unmeasured count at the point of entry instead, and
        // this test is the proof that the slot still produces.
        let mut scores = run("v", 2 * ADVERSARY_MIN_SAMPLE, -20.0, 0);
        scores.push(score("v", Some(f64::NAN), 500));
        scores.push(score("v", Some(f64::INFINITY), 501));
        let profiles = assess(&scores);
        let profile = profiles.get("v").expect("the venue is profiled");
        assert_eq!(profile.sample, 2 * ADVERSARY_MIN_SAMPLE + 2);
        assert_eq!(
            profile.measured,
            2 * ADVERSARY_MIN_SAMPLE,
            "a non-finite error was measured"
        );
        let produced = slot(&scores, Timestamp::from_secs(10_000));
        assert!(
            produced.value().is_some(),
            "the slot did not produce, so a non-finite reading reached the encoder"
        );
    }

    #[test]
    fn venues_are_profiled_separately_and_in_name_order() {
        // The replay property. Two venues, one deteriorating and one not, and
        // the profile for each is about that venue's own fills — an
        // implementation that pooled them would report both as the average of
        // the two. `BTreeMap` order is asserted because this map is
        // serialised into a signed slot: a map that iterated in two orders
        // would sign as two payloads.
        let mut scores = run("zeta", ADVERSARY_MIN_SAMPLE, -20.0, 0);
        scores.extend(run("alpha", ADVERSARY_MIN_SAMPLE, 1.0, 100));
        let profiles = assess(&scores);
        assert_eq!(
            profiles.keys().collect::<Vec<_>>(),
            vec!["alpha", "zeta"],
            "the profiles are not in name order"
        );
        assert_eq!(
            profiles["alpha"].posture,
            AdversaryPosture::Benign,
            "a venue filling better than modelled was not benign"
        );
        assert_eq!(profiles["zeta"].posture, AdversaryPosture::Deteriorating);
    }

    #[test]
    fn the_window_keeps_only_the_most_recent_scores_so_the_working_set_is_bounded() {
        // Bounded retention. Twice the window of fills at a venue that used
        // to fill badly, followed by a window of fills at a venue that fills
        // well: the old venue must have fallen out entirely, or the monitor's
        // working set grows with the platform's history.
        let mut scores = run("old", 2 * ADVERSARY_WINDOW, -50.0, 0);
        scores.extend(run("new", ADVERSARY_WINDOW, 0.0, 10_000));
        assert_eq!(
            scores.len(),
            3 * ADVERSARY_WINDOW,
            "the premise failed: the fixture is not longer than the window"
        );
        let profiles = assess(&scores);
        assert_eq!(
            profiles.keys().collect::<Vec<_>>(),
            vec!["new"],
            "a venue outside the window is still profiled"
        );
        assert_eq!(profiles["new"].sample, ADVERSARY_WINDOW);
    }

    #[test]
    fn a_slot_is_produced_only_once_some_venue_has_actually_been_measured() {
        // The fail-closed half of slot 12. Nine fills is below the bar, so
        // nothing is measured and nothing may be shipped: a produced slot
        // reads `Fresh` at a cell, and a slot that turned fresh on the
        // strength of the producer having *run* is the shape of claim this
        // platform refuses everywhere else.
        let now = Timestamp::from_secs(10_000);
        let thin = run("v", ADVERSARY_MIN_SAMPLE - 1, -20.0, 0);
        assert_eq!(
            assess(&thin)["v"].posture,
            AdversaryPosture::Unmeasured,
            "the premise failed: the thin fixture was measured"
        );
        assert_eq!(
            slot(&thin, now).freshness(PolicyItem::AdversaryProfiles, now),
            qip_contracts::degradation::Freshness::Unavailable
        );

        let enough = run("v", ADVERSARY_MIN_SAMPLE, -20.0, 0);
        let produced = slot(&enough, now);
        assert_eq!(
            produced.freshness(PolicyItem::AdversaryProfiles, now),
            qip_contracts::degradation::Freshness::Fresh
        );
        let venues = &produced
            .value()
            .expect("a produced slot carries a value")
            .venues;
        assert_eq!(venues.keys().collect::<Vec<_>>(), vec!["v"]);
        assert_eq!(
            venues["v"]
                .get("posture")
                .and_then(serde_json::Value::as_str),
            Some("deteriorating"),
            "the shipped profile does not carry the posture the monitor found"
        );
    }

    #[test]
    fn an_empty_score_list_profiles_nothing_rather_than_panicking_on_the_window() {
        // The slice arithmetic in `assess` subtracts the window from the
        // length; on an empty list that must saturate rather than wrap.
        assert!(assess(&[]).is_empty());
        let now = Timestamp::from_secs(10_000);
        assert_eq!(
            slot(&[], now).freshness(PolicyItem::AdversaryProfiles, now),
            qip_contracts::degradation::Freshness::Unavailable
        );
    }

    #[test]
    fn a_posture_change_is_keyed_so_a_retry_in_one_cycle_cannot_write_it_twice() {
        // The idempotency contract `journal_once` reads. Two records for the
        // same venue and posture in one cycle share a key; the same venue
        // returning to that posture in a later cycle does not, or a venue
        // that went benign, adapted, and adapted again would have its second
        // adaptation silently swallowed.
        let profile = profile("v", 20, &[10.0; 20]);
        let first = AdversaryPostureChanged::of(
            &profile,
            AdversaryPosture::Benign,
            7,
            Timestamp::from_secs(1),
        );
        let retry = AdversaryPostureChanged::of(
            &profile,
            AdversaryPosture::Benign,
            7,
            Timestamp::from_secs(2),
        );
        let later = AdversaryPostureChanged::of(
            &profile,
            AdversaryPosture::Benign,
            8,
            Timestamp::from_secs(3),
        );
        assert_eq!(first.idempotency_key(), retry.idempotency_key());
        assert_ne!(first.idempotency_key(), later.idempotency_key());
        assert_eq!(
            first.idempotency_key().as_deref(),
            Some("adversary-posture:v:deteriorating:7"),
            "the key does not name the venue, the posture and the cycle"
        );
    }
}
