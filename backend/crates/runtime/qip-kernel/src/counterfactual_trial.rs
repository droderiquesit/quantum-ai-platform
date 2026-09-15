//! Blueprint §12.4's third guardrail: "counterfactual findings enter the same
//! statistical gate as any other hypothesis, with the same trial accounting".
//!
//! # What was wrong with the bar this replaces
//!
//! ADR 0055 set one fixed threshold — ten observations, three quarters of
//! them one way — and `rule_review`, `sizing_review` and `venue_review` all
//! read it by name so the platform would have one answer to "how much
//! evidence makes a pattern a finding". That is a good property and it is
//! kept. What it never was is a *statistical* gate, and the difference is the
//! whole of this module: a fixed threshold is a test the platform may run as
//! many times as it likes at no cost. Seven risk rules reviewed every cycle,
//! against a 256-entry window that turns over as scores land, is hundreds of
//! tests a day of the same hypothesis shape. Ten flips landing eight-and-two
//! happens by chance about once in nineteen; a platform that looks nineteen
//! times will see it, propose a loosening of the rule it saw it on, and put
//! two operators in front of a signature page holding evidence that is noise.
//! That is exactly the row's own risk — "overfitting to counterfactual
//! results" — and a threshold nobody charges for cannot see it, because the
//! number that would show it is the number of looks, and nothing counted
//! them.
//!
//! # The gate
//!
//! A finding is `supporting` of `sample` scored paths pointing one way. The
//! null is that a scored path is as likely to favour the finding as not — a
//! coin, `p₀ = 0.5`. It is the weakest null available and that is why it is
//! used: any stronger one (that a gate is right three times in five, say)
//! would be a figure nobody in this tree has measured, and a null fitted to
//! make findings clear is the failure ADR 0054 names for a different
//! threshold. The one-sided tail is read off the normal approximation to the
//! binomial with a continuity correction, through
//! [`qip_numerics::distributions::normal_cdf`] — no new dependency, and the
//! same function `deflated_sharpe` already reads.
//!
//! The correction for having looked before is Bonferroni against the
//! **quarter's** charged trials: the bar a finding must clear is
//! [`COUNTERFACTUAL_FAMILY_WISE_ALPHA`] divided by the number of looks
//! charged in the calendar quarter, this one included. Quarterly rather than
//! lifetime, and the choice is load-bearing in both directions:
//!
//! * Lifetime would drive the bar to zero. After a hundred thousand looks no
//!   finite sample in a 256-entry window clears `5e-7`, so the gate would
//!   stop being able to admit anything at all — a control that can only
//!   refuse, which `.claude/rules/domains/infrastructure.md` names as the
//!   thing that distinguishes a working gate from one that refuses
//!   everything, and which is the `MaxExpectedShortfall` defect wearing
//!   the opposite sign.
//! * Per-cycle, or per-review, would be no correction at all: a window that
//!   resets faster than the evidence turns over is a fresh alpha for every
//!   look, which is the fixed threshold again under a p-value's name.
//!
//! The quarter is also the window the trial book already budgets in
//! (`qip_lifecycle::trials::DEFAULT_QUARTERLY_BUDGET`, blueprint §20.1), so
//! the correction and the budget are counted off the same record rather than
//! from two clocks that can disagree.
//!
//! # Which findings come here, and which deliberately do not
//!
//! Every counterfactual finding that asks a **control to move** is charged
//! and judged here: `Platform::review_rules`' recalibration proposal (§12.3's
//! first row — the only counterfactual finding in this tree that can end in a
//! loosened risk bound) and `Platform::review_sizing`' larger-size proposal
//! (ADR 0063's loosening half). In both, a refusal leaves the control exactly
//! where it was and leaves nothing for anybody to sign, which is the closed
//! direction.
//!
//! `Platform::counterfactual_sizing_multiplier` and
//! `sizing_review::cap_multiplier` are deliberately **not** gated here, and
//! the reason is the direction of their refusal rather than a judgement that
//! they matter less. Both can only narrow: their finding halves what the
//! platform will size into. A trial gate in front of either would be a
//! statistical test whose refusal makes the platform trade *larger* — a gate
//! that can only loosen, which is the one thing §12.4's fourth guardrail
//! forbids outright. A defence (§12.3's second row) is not gated either: it
//! records that a rule earned its place, moves nothing, and is re-derived
//! whenever a new score lands, so charging it would spend on restatement the
//! budget the proposals need.
//!
//! # Money
//!
//! Nothing here is money and nothing here is a [`qip_core::Decimal`]. Counts,
//! a tail probability and an alpha, all `f64`, in the statistics lane; the
//! crossing point into `Decimal` is elsewhere, in `sizing_confidence`, and
//! this module's verdict is a `bool` on the way to a record.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_lifecycle::trials::TrialAccount;
use qip_numerics::distributions::normal_cdf;

/// The family-wise error rate the corrected bar is spread across.
///
/// Five per cent, the conventional figure, and stated rather than tuned: a
/// platform that moved it after seeing which findings it refused would be
/// doing the thing this module exists to stop, one level up.
pub const COUNTERFACTUAL_FAMILY_WISE_ALPHA: f64 = 0.05;

/// The null this gate tests against: a scored path favours the finding as
/// often as not.
const NULL_SHARE: f64 = 0.5;

/// `outcome` label values on [`qip_observability::metrics::names::COUNTERFACTUAL_TRIALS`].
/// Three source-file literals, so the label's cardinality is fixed by this
/// file and not by anything a finding carries.
pub mod outcome {
    /// The finding cleared the corrected bar and the record was allowed.
    pub const ADMITTED: &str = "admitted";
    /// The finding was charged a trial and did not clear the corrected bar.
    pub const REFUSED: &str = "refused";
    /// No trial could be charged at all — the family's quarterly budget is
    /// spent, or the subject cannot be named — so the finding was refused
    /// without being tested.
    pub const UNCHARGED: &str = "uncharged";
}

/// One counterfactual finding, charged and judged.
#[derive(Clone, Debug, PartialEq)]
pub struct CounterfactualTrial {
    /// The rule or instrument the finding is about.
    pub subject: String,
    /// Scored paths in the finding's sample.
    pub sample: usize,
    /// Of those, how many point the finding's way.
    pub supporting: usize,
    /// One-sided tail probability of seeing `supporting` or more under
    /// [`NULL_SHARE`].
    pub p_value: f64,
    /// [`COUNTERFACTUAL_FAMILY_WISE_ALPHA`] divided by [`Self::looks`].
    pub alpha: f64,
    /// Trials charged to the counterfactual family in this calendar quarter,
    /// this one included.
    pub looks: u64,
    /// The family's lifetime count, carried so a reader can see how long the
    /// platform has been looking beyond the quarter it is correcting in.
    pub lifetime: u64,
    pub charged_at: Timestamp,
    /// Whether the finding may be recorded and acted on.
    pub admitted: bool,
}

impl CounterfactualTrial {
    /// The `outcome` label this trial is counted under.
    pub fn outcome(&self) -> &'static str {
        if self.admitted {
            outcome::ADMITTED
        } else {
            outcome::REFUSED
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} of {} scored path(s) on {} give a one-sided p of {:.5} against a bar of {:.5} \
             ({:.3} spread over {} look(s) charged this quarter, {} lifetime)",
            self.supporting,
            self.sample,
            self.subject,
            self.p_value,
            self.alpha,
            COUNTERFACTUAL_FAMILY_WISE_ALPHA,
            self.looks,
            self.lifetime
        )
    }
}

/// The one-sided probability of `supporting` or more of `sample` paths
/// pointing one way, if each pointed that way by a coin flip.
///
/// Normal approximation with the half-unit continuity correction, which is
/// what makes the small samples this platform actually has — ten to a few
/// dozen — read close to the exact binomial rather than optimistically. It is
/// an approximation and says so; the direction of its error at these sizes is
/// slightly conservative on the tail that matters, which is the direction a
/// guardrail should err in.
///
/// Refuses rather than clamping: a sample of nothing has no tail, and more
/// supporting paths than paths is a caller that has miscounted, which a
/// clamped `1.0` would hide behind a finding that always clears.
pub fn one_sided_p(sample: usize, supporting: usize) -> Result<f64> {
    if sample == 0 {
        return Err(Error::invalid(
            "a counterfactual finding with no scored paths has no significance to compute; the \
             sample bar is checked before this is called",
        ));
    }
    if supporting > sample {
        return Err(Error::invalid(format!(
            "{supporting} supporting path(s) out of a sample of {sample}: the finding counts more \
             evidence than it has, so its p-value would be a number about nothing"
        )));
    }
    // usize → f64: counts crossing into the statistics lane, where they stay.
    let n = sample as f64;
    let k = supporting as f64;
    let standard_deviation = (n * NULL_SHARE * (1.0 - NULL_SHARE)).sqrt();
    if standard_deviation <= 0.0 {
        return Err(Error::numeric(
            "the null's standard deviation is not positive, so no tail can be read",
        ));
    }
    let z = (k - 0.5 - n * NULL_SHARE) / standard_deviation;
    let p = 1.0 - normal_cdf(z);
    if !p.is_finite() {
        return Err(Error::numeric(format!(
            "the one-sided tail for {supporting} of {sample} is not finite"
        )));
    }
    Ok(p)
}

/// The bar one finding must clear, given how many times the platform has
/// looked this quarter.
///
/// `looks` is the charged count including this finding's own trial, so it is
/// never zero on the path this is reached from; the `max(1)` is here so that a
/// caller reading a deserialised account cannot produce a division by zero and
/// an infinite bar that admits everything.
pub fn corrected_alpha(looks: u64) -> f64 {
    // u64 → f64: a count crossing into the statistics lane.
    COUNTERFACTUAL_FAMILY_WISE_ALPHA / looks.max(1) as f64
}

/// Judge one finding against the account its trial was charged under.
///
/// Separate from the charging so that the arithmetic can be exercised without
/// a book, and so that the seam that charges is the one place a trial is
/// spent. The account must be the one this finding's charge produced —
/// `Platform::counterfactual_trial` is the only production caller and passes
/// exactly that.
pub fn verdict(
    subject: &str,
    sample: usize,
    supporting: usize,
    account: &TrialAccount,
) -> Result<CounterfactualTrial> {
    let p_value = one_sided_p(sample, supporting)?;
    let looks = account.quarter_trials();
    let alpha = corrected_alpha(looks);
    Ok(CounterfactualTrial {
        subject: subject.to_string(),
        sample,
        supporting,
        p_value,
        alpha,
        looks,
        lifetime: account.lifetime(),
        charged_at: account.charged_at(),
        admitted: p_value <= alpha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{
        COUNTERFACTUAL_SIZING_MIN_SAMPLE, COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION,
    };
    use qip_lifecycle::trials::TrialBook;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    /// A book with `looks - 1` trials already charged to the counterfactual
    /// family, then one more for the finding under test, so the account
    /// carries exactly `looks`.
    fn account_after(looks: u64) -> TrialAccount {
        let mut book = TrialBook::in_memory();
        let mut last = None;
        for index in 0..looks {
            last = Some(
                book.charge_counterfactual(&format!("subject-{index}"), at())
                    .expect("a look is charged"),
            );
        }
        last.expect("at least one look")
    }

    #[test]
    fn a_unanimous_sample_at_the_fixed_bar_clears_the_first_look() {
        // The premise: ten of ten is exactly the sample ADR 0055's fixed
        // threshold admits, so if this did not clear, the gate would have
        // refused everything the old rule admitted and be a gate that only
        // refuses.
        let account = account_after(1);
        assert_eq!(
            account.quarter_trials(),
            1,
            "the premise failed: the account is not the first look"
        );
        let trial = verdict(
            "order-notional",
            COUNTERFACTUAL_SIZING_MIN_SAMPLE,
            COUNTERFACTUAL_SIZING_MIN_SAMPLE,
            &account,
        )
        .expect("a unanimous sample is scoreable");
        assert!(
            trial.admitted,
            "a unanimous sample of ten did not clear the first look: {}",
            trial.describe()
        );
    }

    #[test]
    fn the_same_unanimous_sample_stops_clearing_once_the_platform_has_looked_a_hundred_times() {
        // The failure this prevents: a fixed threshold is free to re-run, so
        // a platform that reviews seven rules every cycle finds one clearing
        // ten-of-ten eventually and proposes a loosening on it.
        let first = account_after(1);
        let hundredth = account_after(100);
        assert_eq!(
            hundredth.quarter_trials(),
            100,
            "the premise failed: the book did not charge a hundred looks"
        );
        let early = verdict("order-notional", 10, 10, &first).expect("scoreable");
        assert!(
            early.admitted,
            "the premise failed: the same evidence does not clear at the first look either"
        );
        let late = verdict("order-notional", 10, 10, &hundredth).expect("scoreable");
        assert!(
            !late.admitted,
            "a hundred looks did not raise the bar: {}",
            late.describe()
        );
        assert!(
            late.alpha < early.alpha,
            "the corrected bar did not tighten: {} against {}",
            late.alpha,
            early.alpha
        );
    }

    #[test]
    fn eight_of_ten_clears_the_fixed_threshold_and_does_not_clear_the_trial_gate() {
        // The premise, asserted first: eight of ten is what ADR 0055's fixed
        // rule admits — sample at the bar, fraction 0.8 over 0.75 — so this
        // test is about the statistical gate refusing evidence the platform
        // used to act on, not about a sample that never qualified.
        let sample = COUNTERFACTUAL_SIZING_MIN_SAMPLE;
        let supporting = 8;
        assert!(sample >= COUNTERFACTUAL_SIZING_MIN_SAMPLE);
        assert!(
            supporting as f64 / sample as f64 >= COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION,
            "the premise failed: eight of ten does not clear ADR 0055's fraction"
        );
        let account = account_after(1);
        let trial = verdict("order-notional", sample, supporting, &account).expect("scoreable");
        assert!(
            !trial.admitted,
            "eight of ten cleared even the uncorrected bar: {}",
            trial.describe()
        );
        // And a bigger sample at the same fraction does clear, so the refusal
        // above is about the evidence being thin and not about the fraction
        // being unreachable.
        let earned = verdict("order-notional", 80, 60, &account).expect("scoreable");
        assert!(
            earned.admitted,
            "sixty of eighty at the same fraction did not clear: {}",
            earned.describe()
        );
    }

    #[test]
    fn a_finding_that_counts_more_evidence_than_it_has_is_refused_rather_than_scored() {
        let error = one_sided_p(10, 11).expect_err("eleven of ten was scored");
        assert!(
            error.message().contains("more evidence than it has"),
            "the refusal does not name the miscount: {}",
            error.message()
        );
        let empty = one_sided_p(0, 0).expect_err("an empty sample was scored");
        assert!(
            empty.message().contains("no scored paths"),
            "the refusal does not name the empty sample: {}",
            empty.message()
        );
    }

    #[test]
    fn the_corrected_bar_never_divides_by_a_look_count_of_zero() {
        // A deserialised account can carry any quarter count, including zero;
        // an infinite bar admits every finding, which is the gate silently
        // switched off.
        assert!(corrected_alpha(0).is_finite());
        assert!(
            (corrected_alpha(0) - COUNTERFACTUAL_FAMILY_WISE_ALPHA).abs() < f64::EPSILON,
            "a look count of zero did not fall back to the uncorrected alpha: {}",
            corrected_alpha(0)
        );
    }
}
