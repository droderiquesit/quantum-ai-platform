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
//! flow, and hypothesis-plus-falsification; confounder adjustment for this
//! method too).

use qip_core::{Duration, Error, Result, Timestamp};
use qip_numerics::stats::granger_causality;

use crate::causal::{CausalEdge, Mechanism};

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
/// establishes precedence, not a mechanism, and §9.4's "confounders are
/// often unobserved" limit applies to every edge it can produce without
/// exception, because nothing here adjusts for one.
pub const TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING: f64 = 0.5;

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
///   ([`granger_causality`]'s own refusals: mismatched lengths, a
///   non-finite value).
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

    let test = granger_causality(cause_returns, effect_returns, TEMPORAL_PRECEDENCE_LAG)?;
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
    let confidence =
        ((1.0 - test.p_value) * TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING).clamp(0.0, 1.0);
    let lag = bar_interval * TEMPORAL_PRECEDENCE_LAG as i64;
    let evidence_id = format!(
        "granger:{cause_id}->{effect_id}:lag={}:p={:.4}:n={}",
        TEMPORAL_PRECEDENCE_LAG, test.p_value, test.observations
    );

    Ok(Some(
        CausalEdge::new(
            cause_id,
            effect_id,
            mechanism,
            test.partial_r_squared,
            lag,
            recorded_at,
        )
        .with_confidence(confidence)
        .with_evidence(vec![evidence_id]),
    ))
}
