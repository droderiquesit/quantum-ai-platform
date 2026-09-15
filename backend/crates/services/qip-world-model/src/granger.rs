//! Establishing a causal edge from temporal precedence — blueprint §9.2's
//! Granger-style method, the one of the six named establishment methods this
//! platform can compute honestly from data it already ingests.
//!
//! `CausalEdge::new` and `WorldModel::claim_causal` have existed since this
//! crate did; what has not existed is a second caller that reaches them from
//! real evidence rather than `world::seed_demo_world`'s hand-written demo
//! claims. This module is that caller's statistics, kept separate from
//! [`crate::causal`] because a re-estimation of an *existing* edge
//! ([`crate::causal::CausalGraph::reestimate`]) and the decision to *create*
//! one are different questions with different bars: re-estimation trusts a
//! link a person already asserted with a mechanism; this module has no
//! mechanism to trust and must clear a statistical bar on its own before
//! writing anything at all.
//!
//! See ADR-0054 for why Granger-style lead-lag was chosen over the other five
//! methods §9.2 names, and for what remains open (natural experiments,
//! instrumental variables, structural constraints, the platform's own order
//! flow, and hypothesis-plus-falsification).
//!
//! **Confounder adjustment is no longer among them.** ADR-0054 listed it as
//! open and it was: this module ran an uncontrolled test, which §9.2 does not
//! actually name — the blueprint's method is "Granger-style lead-lag *with
//! controls*". [`establish_temporal_precedence_controlling_for`] is that
//! method, and [`establish_temporal_precedence`] is now a thin call to it
//! with an empty [`crate::confounder::ConfounderSet`]. The uncontrolled form
//! is kept because a pair with no plausible common driver is a real case,
//! and removed as the default because a scan of one book under a shared
//! driver manufactures an edge between nearly every pair that driver
//! touches. See [`crate::confounder`].

use qip_core::{Duration, Error, Result, Timestamp};
use qip_numerics::stats::granger_causality_controlling_for;

use crate::causal::{CausalEdge, Mechanism};
use crate::confounder::ConfounderSet;

/// The lag tested, in bars of the effect series' own cadence. Fixed at one
/// rather than swept over several — see
/// [`qip_numerics::stats::GrangerCausalityTest`] for why a single lag is what
/// keeps the coefficient's sign, and therefore the choice between
/// [`Mechanism::TemporalPrecedence`] and [`Mechanism::InverseTemporalPrecedence`],
/// unambiguous.
pub const TEMPORAL_PRECEDENCE_LAG: usize = 1;

/// Bars of return history required before a pair is tested at all.
///
/// Below this an F-test built on `2*lag + 1` parameters has too few degrees
/// of freedom for its own large-sample justification to be trusted, whatever
/// p-value it reports — 60 leaves 58 residual degrees of freedom for the
/// unrestricted fit at `lag = 1`, comfortably inside where the F
/// approximation is standard practice.
pub const TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS: usize = 60;

/// Significance bar for writing an edge.
///
/// Strict rather than the conventional 0.05: this method is meant to be run
/// over many instrument pairs, and at 0.05 roughly one pair in twenty clears
/// the bar on pure noise alone — which is exactly the false-positive shape
/// [`establish_temporal_precedence`]'s own doc comment and its refusal test
/// exist to keep out of the graph. Not a multiple-comparisons correction —
/// that needs a stated family size this module does not track, because the
/// caller decides how many pairs it tests per pass — a floor that keeps an
/// uncorrected false-positive rate an order of magnitude below the
/// conventional one.
pub const TEMPORAL_PRECEDENCE_ALPHA: f64 = 0.01;

/// Minimum partial R² to write an edge even below the significance bar.
///
/// Enough bars make a trivial effect clear `p < 0.01` while explaining
/// almost nothing; this refuses that case rather than sizing against a
/// rounding error wearing a confident-looking p-value.
pub const TEMPORAL_PRECEDENCE_MIN_EFFECT: f64 = 0.02;

/// The confidence ceiling for a temporal-precedence edge, whatever its
/// p-value.
///
/// `seed_demo_world`'s mechanism-backed, evidence-cited claims default to
/// `0.7` ([`CausalEdge::new`]). This method may never reach that: it
/// establishes precedence, not a mechanism, and no amount of adjustment
/// turns a lead-lag relationship into a channel anybody can name.
///
/// This applies to an edge produced under a set of observed controls with
/// nothing recorded as unobserved. Where a plausible unobserved confounder
/// *is* recorded, [`TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING`] applies
/// instead.
pub const TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING: f64 = 0.5;

/// The confidence ceiling for an edge against which a plausible *unobserved*
/// confounder is recorded — blueprint §9.4's "suggestive rather than
/// established".
///
/// **This number was not estimated from anything, and saying so is the
/// point.** Nothing in this platform measures how much an unnamed common
/// cause should cost a claim; a figure presented here as though it had been
/// inferred would be a fabricated measurement wearing a constant's clothes.
/// What is asserted, and what does not need measuring, is the *ordering*: an
/// edge carrying a confounder nobody could adjust for must never rank above
/// an edge produced by the same statistics with that confounder removed.
/// Half the adjusted ceiling is one point satisfying that ordering, and
/// a `const` assertion below holds `SUGGESTIVE < CONFIDENCE` at compile time,
/// and no test asserts either value, so that changing the point does not
/// quietly become changing the claim.
pub const TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING: f64 =
    TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING / 2.0;

/// The ordering above, enforced by the compiler rather than by a test.
///
/// A guarantee the type system holds beats one a runtime check holds, which
/// beats one a comment asserts. The *ordering* is the claim this module
/// makes — an edge carrying an unadjustable confounder never outranks the
/// same statistics without it — and someone editing either constant to a
/// value that broke it would otherwise find out from a test run, or not at
/// all if they edited both. This fails the build.
const _: () = assert!(
    TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING < TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING,
    "a suggestive edge must never be capable of outranking an adjusted one"
);

/// Establish a causal edge between `cause_id` and `effect_id` from their
/// return histories alone, or refuse.
///
/// # What is returned, and why
///
/// * `Ok(Some(edge))` — the test cleared [`TEMPORAL_PRECEDENCE_ALPHA`] and
///   [`TEMPORAL_PRECEDENCE_MIN_EFFECT`]. The edge's mechanism is
///   [`Mechanism::TemporalPrecedence`] or its inverse depending on the sign
///   of the test's coefficient, its `strength` is the test's partial R² —
///   the only bounded quantity the test itself produces — and its
///   `confidence` is `(1 - p_value)` scaled into
///   `[0, TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING]`.
/// * `Ok(None)` — a legitimate, ordinary "no": too little history yet
///   (normal early in a deployment's life, not a bug), or a test that ran
///   and did not clear the bar. Refusing quietly here, not with an error, is
///   deliberate: a caller scanning many pairs every cycle must not treat
///   "this pair had nothing to say" as a fault.
/// * `Err` — a caller bug: `cause_id == effect_id`, or a malformed series
///   ([`qip_numerics::stats::granger_causality_controlling_for`]'s own refusals:
///   mismatched lengths, a non-finite value).
///
/// # `bar_interval` is the caller's fact, not this function's assumption
///
/// The edge's `lag` field is a [`Duration`] — "typical delay before the
/// effect is observable" — and the only Duration this function can name
/// truthfully is [`TEMPORAL_PRECEDENCE_LAG`] multiplied by however much wall
/// time separates one return observation from the next in the series it was
/// given. That is a fact about the caller's bars, not about statistics, so
/// it is a parameter rather than a constant baked in here — inventing a
/// default (a day, say) would silently mislabel a claim built from
/// intraday bars.
pub fn establish_temporal_precedence(
    cause_id: &str,
    cause_returns: &[f64],
    effect_id: &str,
    effect_returns: &[f64],
    bar_interval: Duration,
    recorded_at: Timestamp,
) -> Result<Option<CausalEdge>> {
    establish_temporal_precedence_controlling_for(
        cause_id,
        cause_returns,
        effect_id,
        effect_returns,
        &ConfounderSet::new(),
        bar_interval,
        recorded_at,
    )
}

/// [`establish_temporal_precedence`] with blueprint §9.2's own qualifier
/// attached: "temporal precedence with confounders **explicitly adjusted**".
///
/// # What `confounders` changes
///
/// Every [`crate::confounder::Confounder::observed`] in the set contributes its lags to both
/// the restricted and unrestricted regressions
/// ([`qip_numerics::stats::granger_causality_controlling_for`]), so the
/// F-test measures the cause's lagged information about the effect's future
/// *beyond* what those drivers already carry. An uncontrolled scan over one
/// book reproduces any shared driver as an edge between very many of the
/// pairs it touches; each is significant, each is spurious, and they fail
/// together when the regime turns — which is the failure §9 was written to
/// answer, so running this method without controls does not merely weaken it.
///
/// Every [`crate::confounder::Confounder::unobserved`] in the set adjusts for nothing and is
/// carried onto the edge's `suspected_confounders`, making it
/// [`crate::causal::EdgeStanding::Suggestive`] — §9.4's "recorded as such,
/// and the edge is treated as suggestive rather than established".
///
/// # The confidence ceiling is a chosen ordering, not an estimate
///
/// A suggestive edge is capped at
/// [`TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING`] and an adjusted one at
/// [`TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING`]. **Neither number was
/// estimated from anything.** No procedure here measures how much an
/// unobserved confounder should cost a claim, and presenting one of these as
/// if it had been inferred would be the dishonest half of the same act. What
/// they encode is an *ordering* the platform is entitled to assert without
/// measuring anything: an edge with a named confounder nobody could adjust
/// for must never outrank an edge established under the same statistics with
/// that confounder removed. The ordering is the property; the numbers are
/// one pair of points that satisfies it, and the tests assert the ordering.
///
/// # Refusals
///
/// Everything [`establish_temporal_precedence`] refuses, plus a control
/// series not sampled on the same bars as the pair — refused by name here,
/// rather than as an anonymous column index out of the numerics layer, so
/// the message says which driver is mis-sampled.
pub fn establish_temporal_precedence_controlling_for(
    cause_id: &str,
    cause_returns: &[f64],
    effect_id: &str,
    effect_returns: &[f64],
    confounders: &ConfounderSet,
    bar_interval: Duration,
    recorded_at: Timestamp,
) -> Result<Option<CausalEdge>> {
    if cause_id == effect_id {
        return Err(Error::invalid(format!(
            "{cause_id} cannot Granger-cause itself; pass two distinct series"
        )));
    }
    if cause_returns.len() < TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS
        || effect_returns.len() < TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS
    {
        // Not enough history yet is a normal, ongoing state — not a caller
        // bug — so this refuses quietly rather than with an error naming a
        // threshold the platform's own feed will clear as more bars arrive.
        return Ok(None);
    }

    // Refused by name rather than by column index: the numerics layer can
    // only say "control series 2", and an operator reading that has to
    // reconstruct the set's ordering to learn which driver is wrong.
    if let Some(id) = confounders.misaligned(cause_returns.len()) {
        return Err(Error::invalid(format!(
            "confounder '{id}' is not sampled on the same bars as {cause_id}->{effect_id} \
             ({} observation(s)); resample it or leave it out — a control on the wrong \
             instants adjusts for the wrong thing",
            cause_returns.len()
        )));
    }

    let controls = confounders.observed_series();
    let test = granger_causality_controlling_for(
        cause_returns,
        effect_returns,
        &controls,
        TEMPORAL_PRECEDENCE_LAG,
    )?;
    if test.p_value >= TEMPORAL_PRECEDENCE_ALPHA
        || test.partial_r_squared < TEMPORAL_PRECEDENCE_MIN_EFFECT
        || test.coefficient == 0.0
    {
        return Ok(None);
    }

    let mechanism = if test.coefficient > 0.0 {
        Mechanism::TemporalPrecedence
    } else {
        Mechanism::InverseTemporalPrecedence
    };
    let adjusted_for = confounders.observed_ids();
    let suspected = confounders.unobserved_ids();
    // §9.4's ordering, made arithmetic. See this function's doc comment: the
    // ceiling is a chosen point, the ordering between the two is the claim.
    let ceiling = if suspected.is_empty() {
        TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING
    } else {
        TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING
    };
    // Not clamped. A p-value outside [0, 1], or one that is not a number at
    // all, is a defect in the F distribution's tail and not a confidence to be
    // corrected into range — and the clamp that stood here could not have
    // corrected the case that actually reaches this line, because
    // `NAN.clamp(0.0, 1.0)` is `NAN`. `with_confidence` refuses it below,
    // naming the link, which is the message an operator can act on.
    let confidence = (1.0 - test.p_value) * ceiling;
    let lag = bar_interval * TEMPORAL_PRECEDENCE_LAG as i64;
    // What was controlled for goes into the evidence string, not only into
    // the edge's own fields. An operator comparing two edges in a log reads
    // the evidence id; one that omitted the controls would make an adjusted
    // edge and an unadjusted one indistinguishable in the one place they are
    // most often compared.
    let controlled_for = if adjusted_for.is_empty() {
        "none".to_string()
    } else {
        adjusted_for.iter().cloned().collect::<Vec<_>>().join("+")
    };
    let evidence_id = format!(
        "granger:{cause_id}->{effect_id}:lag={}:p={:.4}:n={}:controls={controlled_for}",
        TEMPORAL_PRECEDENCE_LAG, test.p_value, test.observations
    );

    // Both fractions are refused rather than clamped, and this is the seam
    // where that matters most: `partial_r_squared` comes out of a regression,
    // and the `partial_r_squared < TEMPORAL_PRECEDENCE_MIN_EFFECT` guard above
    // is `false` for a NaN — every comparison against a NaN is — so a
    // degenerate fit does not leave by the quiet `Ok(None)` route. It arrives
    // here, and it stops here with a message naming the pair.
    Ok(Some(
        CausalEdge::new(
            cause_id,
            effect_id,
            mechanism,
            test.partial_r_squared,
            lag,
            recorded_at,
        )?
        .with_confidence(confidence)?
        .with_evidence(vec![evidence_id])
        .with_confounders(adjusted_for, suspected),
    ))
}
