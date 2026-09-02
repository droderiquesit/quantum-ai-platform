//! Names of the series the kernel records that `qip_observability::metrics::names`
//! does not yet spell.
//!
//! Every other name the loop records lives in that list, and for the reason it
//! gives: a name only the call site knows is a name a dashboard query and the
//! registry can each spell differently without anything noticing. These are
//! declared here rather than there only because that module is a library
//! outside the change that introduced them; moving them across is a one-line
//! edit per constant and should happen the next time that file is open. Until
//! then the kernel's `describe_metrics` registers every one of them, so the
//! export carries a `# HELP` line for each.
//!
//! Labels on every series below are bounded by an enum or a source-file
//! literal, never by an instrument, a hypothesis id or an order id.

/// The Brier score over the platform's window of resolved theses — when it
/// said seventy percent, did it happen seventy percent. A gauge, because it
/// is a property of the window as it stands rather than a rate of anything.
pub const BELIEF_BRIER_SCORE: &str = "qip_belief_brier_score";
/// The factor future confidences would be scaled by to match outcomes. One is
/// calibrated; below one the platform is overconfident.
pub const BELIEF_CONFIDENCE_ADJUSTMENT: &str = "qip_belief_confidence_adjustment";
/// How many informative evaluations the two gauges above rest on. A Brier
/// score from three theses is not a Brier score, and this is what says so.
pub const BELIEF_EVALUATIONS: &str = "qip_belief_evaluations";
/// Theses scored against what was published, by `verdict` — the learning
/// engine's six-arm enum, so the label set is closed.
pub const THESES_EVALUATED: &str = "qip_theses_evaluated_total";

/// Declined paths priced by the twin, by the `gate` that declined them — the
/// same names `qip_orders_refused_total` carries, from the same function.
pub const COUNTERFACTUALS_SCORED: &str = "qip_counterfactuals_scored_total";
/// Declined paths that, priced, would have beaten standing aside, by `gate`.
/// Blueprint §12.3: a rule that vetoes mostly profitable paths is too tight,
/// and this is the numerator of that ratio.
pub const COUNTERFACTUAL_REGRETS: &str = "qip_counterfactual_regrets_total";
/// Declined paths that were due for pricing and left for a later cycle because
/// the per-cycle cap was reached. Counted rather than silently truncated.
pub const COUNTERFACTUALS_DEFERRED: &str = "qip_counterfactuals_deferred_total";
/// Declined paths that will never be priced, by `reason` — the working set was
/// full, or the twin refused the evaluation.
pub const COUNTERFACTUALS_UNSCORED: &str = "qip_counterfactuals_unscored_total";

/// Cell fills the central plane attributed to strategies, by `basis`: the
/// contributor vector the cell shipped, or — for a delta written before the
/// vector existed — the largest contributor the older wire named.
pub const CENTRAL_FILLS_ATTRIBUTED: &str = "qip_central_fills_attributed_total";
/// Internal crosses settled to both contributors' books at the mid.
pub const CENTRAL_CROSSES_SETTLED: &str = "qip_central_crosses_settled_total";
/// Orders and crosses the centre refused to settle, by `kind`. A cross naming
/// two buyers carries no per-strategy size, and splitting it evenly would be a
/// guess wearing the ledger's clothes.
pub const CENTRAL_SETTLEMENTS_REFUSED: &str = "qip_central_settlements_refused_total";
/// Attributions whose decomposition did not close. Must stay at zero; a
/// non-zero here is unexplained P&L on the strategy books.
pub const CENTRAL_ATTRIBUTION_FAILURES: &str = "qip_central_attribution_failures_total";

/// Bridge transfers the platform failed on its own evidence, by `failure` —
/// the bridge ledger's five-arm enum. Today only `source_reorg` is recorded,
/// at the instant a reorganisation withdraws the block a transfer's deposit
/// sat in; a transfer that kept waiting for finality on a block that no
/// longer exists is value the destination could still credit against nothing.
pub const BRIDGE_TRANSFERS_FAILED: &str = "qip_bridge_transfers_failed_total";

/// Instruments in the assembled universe that are unfit to drive a capital
/// decision — a licensing class that permits no production decision, a
/// non-positive price, incoherent risk characteristics, or data quality
/// below the floor. A gauge written once at assembly: the universe does not
/// change under a running platform, and a degraded one should be visible
/// before it produces a bad trade rather than after.
pub const UNIVERSE_NOT_DECISION_GRADE: &str = "qip_universe_not_decision_grade";
