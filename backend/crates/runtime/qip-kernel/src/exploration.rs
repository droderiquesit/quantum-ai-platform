//! The exploration budget as the cycle actually runs it (blueprint §13.2).
//!
//! [`qip_capital::exploration`] holds the substance — the probe types, the
//! selection rule, the bounds and the account. This module is the seam: it
//! reads the mandates for the share, reads the platform's own measures of
//! what it does not know, holds the budget out of return-seeking capital, and
//! reports what it did in a line the DECIDE stage carries.
//!
//! # What the hold does, and why it is a hold
//!
//! The budget is taken out of the [`ReservationLedger`] under one id, so
//! `Platform::deployable_capital` — free balance less active holds — sizes
//! the book against what is left. That is the difference between a budget and
//! a label: §13.2 asks for "a line item in the capital engine, not a side
//! effect", and a number reported beside sizing without being subtracted from
//! it is a side effect with a line item's name. The hold is released and
//! retaken on every pass, so a book that lost money explores less on the next
//! pass rather than continuing against the equity it used to have.
//!
//! # What is probed, and what is honestly not
//!
//! Candidates come from the two things this platform genuinely measures about
//! its own ignorance:
//!
//! * the self model's per-component record — an estimable component whose
//!   accuracy sits near one half is [`ProbeKind::UncertainModel`] ("is this
//!   irreducible or merely unobserved"), and one with too thin a record to
//!   estimate at all is [`ProbeKind::StaleEstimate`];
//! * the twin's scored fills per instrument — an instrument with fewer scored
//!   fills than the sizing review needs has an unmeasured capacity, which is
//!   [`ProbeKind::CapacityAtSize`].
//!
//! * since §9.4's marker landed, [`ProbeKind::RegimeBoundary`] — the share
//!   of a subject's incoming causal edges whose conditions are untested in
//!   the regime it has just entered, handed in by the caller because only a
//!   `Platform` holds both the graph and the marker. It is *not*
//!   `1 - confidence`: a hand-asserted mechanism claim, a precedence edge and
//!   a confounded edge take their confidence from three different ceilings,
//!   so ranking probes on it would rank them by how an edge was established
//!   rather than by how little is known about it.
//!
//! [`ProbeKind::UnfamiliarVenue`] is **deliberately not fed from here**, and
//! that is a statement about the platform rather than about the budget:
//! nothing counts orders per venue, so "unfamiliar" would be a number this
//! module invented. A candidate built on it would be a probe sized against a
//! figure nobody computed, which is the defect this whole lane exists to
//! avoid. `RegimeBoundary` was in the same sentence until
//! [`crate::regime_transition`] computed the fact it was missing.
//!
//! # Nothing takes a probe up, and the report says so
//!
//! There is no exploration execution path: a selected probe is capital held
//! and a question recorded, and when it expires the subject is re-measured
//! and the probe settles as [`ProbeEvidence::Observed`]. Those settlements
//! are counted and never scored — see [`qip_capital::exploration`] — so the
//! selection rule cannot come to believe probing works on evidence that no
//! probing happened. Building the execution path is a separate change, and
//! it is an order path, so it is one somebody reviews.

use qip_capital::exploration::{
    ExplorationBook, ExplorationPlan, MAXIMUM_OPEN_PROBES, ProbeCandidate, ProbeEvidence,
    ProbeKind, ProbeOutcome, budget_for,
};
use qip_capital::ledger::UserLedger;
use qip_capital::reservation::ReservationLedger;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_learning_engine::self_model::{MINIMUM_SAMPLE, SelfModel};
use qip_observability::metrics::{Metrics, labels, names};
use std::collections::BTreeMap;

/// The one id the exploration budget is held under.
///
/// One id and not one per probe: the reservation ledger holds the *budget*,
/// and the split of that budget across probes is the exploration book's
/// business. Two ledgers holding the same money in different shapes is the
/// second-source-of-truth failure, and the louder of the two would be wrong.
pub const HOLD_ID: &str = "exploration-budget";

/// How long a probe's question stays open before it is settled or abandoned.
///
/// A day, matching the hold a constructed proposal takes, so exploration
/// capital cannot be pinned for longer than return-seeking capital can.
pub const PROBE_VALIDITY: Duration = Duration::from_hours(24);

/// What resolving one subject's uncertainty is assumed to be worth, in basis
/// points of the book, per unit of uncertainty.
///
/// **A stated assumption, not a measurement**, and named here so it can be
/// argued with rather than discovered in a formula. The platform measures no
/// per-subject value of information, so every candidate carries the same
/// figure and the ranking is therefore driven by uncertainty and by how
/// rarely a subject has been probed — which is what the selection rule is for.
/// The crate takes a per-candidate value and will use it the moment something
/// here can measure one.
pub const RESOLUTION_VALUE_BPS: f64 = 10.0;

/// Scored fills on one instrument below which its capacity is unmeasured.
///
/// [`crate::sizing_review::SIZING_CAP_MIN_SAMPLE`] by reference, because the
/// platform already has one answer to how many scored fills make a pattern,
/// and a second answer here would be a number nobody could reconcile with it.
pub const CAPACITY_SAMPLE: usize = crate::sizing_review::SIZING_CAP_MIN_SAMPLE;

/// The exploration budget's state across cycles: the account, the descriptor
/// registration, and what the reservation ledger is currently holding for it.
#[derive(Debug, Default)]
pub struct ExplorationDesk {
    book: ExplorationBook,
    described: bool,
    /// What [`HOLD_ID`] currently holds, as this module last left it. Kept so
    /// a release is attempted only where a hold was taken: releasing an id
    /// the ledger does not know is refused, and a refusal recorded every
    /// quiet cycle would bury the real ones.
    held: Decimal,
}

impl ExplorationDesk {
    pub fn new() -> Self {
        Self::default()
    }

    /// The account, for the reports and the tests.
    pub fn book(&self) -> &ExplorationBook {
        &self.book
    }

    /// What is currently withheld from return-seeking capital.
    pub fn held(&self) -> Decimal {
        self.held
    }
}

/// What one pass of the exploration budget did, as one line for the cycle
/// report.
///
/// Never empty and never `Option`. The idle state — no mandate sets a share
/// aside — is the state a deployment is normally in, and a module that
/// returned nothing there would reach no surface at all in exactly the case
/// an operator needs to be able to see.
pub fn review(
    desk: &mut ExplorationDesk,
    reservations: &mut ReservationLedger,
    metrics: &Metrics,
    ledger: &UserLedger,
    self_model: &SelfModel,
    fill_scores: &[crate::platform::FillScore],
    regime_boundaries: &BTreeMap<String, f64>,
    equity: Decimal,
    now: Timestamp,
) -> String {
    if !desk.described {
        describe(metrics);
        desk.described = true;
    }

    // What the platform can currently say about its own ignorance, including
    // the subjects it is now certain about: the settlement below needs to
    // read a subject that has stopped being a candidate, and a subject
    // missing from this map is one that can no longer be measured at all.
    let uncertainties = uncertainty_by_subject(self_model, fill_scores, regime_boundaries);
    let mut notes: Vec<String> = Vec::new();
    settle_due(desk, metrics, &uncertainties, now, &mut notes);

    let budget = match mandate_budget(ledger, equity) {
        Ok(budget) => budget,
        Err(error) => {
            // Fail closed: a budget that cannot be established is no budget,
            // and the hold goes back so the capital is not stranded.
            release(desk, reservations, now, &mut notes);
            record_account(metrics, desk, Decimal::ZERO);
            return format!(
                "exploration: no budget — the mandates do not establish one: {}{}",
                error.message(),
                tail(&notes)
            );
        }
    };

    let held = rehold(desk, reservations, budget, now, &mut notes);
    let candidates = candidates_from(&uncertainties, held, equity, &mut notes);
    let plan = match desk.book.plan(held, &candidates, PROBE_VALIDITY, now) {
        Ok(plan) => plan,
        Err(error) => {
            record_account(metrics, desk, held);
            return format!(
                "exploration: {held} held and nothing planned: {}{}",
                error.message(),
                tail(&notes)
            );
        }
    };
    if let Err(error) = desk.book.open_plan(&plan) {
        record_account(metrics, desk, held);
        return format!(
            "exploration: {held} held and the plan was not opened: {}{}",
            error.message(),
            tail(&notes)
        );
    }
    record_selected(metrics, &plan);
    record_account(metrics, desk, held);
    format!("{}{}", plan.summary(), tail(&notes))
}

/// The descriptors, registered once per process. Registering them on every
/// pass would rewrite the help text of every series on every cycle.
fn describe(metrics: &Metrics) {
    metrics.describe(
        names::EXPLORATION_BUDGET,
        "capital set aside for information gain rather than expected return, and withheld from \
         return-seeking sizing; zero where no mandate sets a share aside",
    );
    metrics.describe(
        names::EXPLORATION_COMMITTED,
        "of the exploration budget, what the live probes have between them put at risk",
    );
    metrics.describe(
        names::EXPLORATION_SPEND,
        "what exploration has actually spent, cumulatively; reported apart from return-seeking \
         capital so the cost of learning does not read as drag",
    );
    metrics.describe(
        names::EXPLORATION_PROBES_SELECTED,
        "probes the upper-confidence-bound rule selected, by probe kind",
    );
    metrics.describe(
        names::EXPLORATION_PROBES_SETTLED,
        "probes closed, by whether the answer came from a probe being taken up or from the \
         subject being re-measured while the probe sat unexercised",
    );
}

/// Every subject this platform can currently measure its ignorance of, with
/// the kind of probe that would resolve it.
///
/// Both sources are already bounded — the self model evicts past
/// `MAX_COMPONENTS`, and the fill scores are bounded by the counterfactual
/// history — so this map is bounded without a cap of its own.
fn uncertainty_by_subject(
    self_model: &SelfModel,
    fill_scores: &[crate::platform::FillScore],
    regime_boundaries: &BTreeMap<String, f64>,
) -> BTreeMap<String, (ProbeKind, f64)> {
    let mut subjects = BTreeMap::new();
    for (key, estimate) in self_model.iter() {
        let sample = estimate.sample_count();
        // usize → f64 on both arms: a sample ratio and an accuracy are
        // statistics, and this is where they stop being counts.
        let (kind, uncertainty) = match estimate.estimate() {
            Ok(capability) => (
                ProbeKind::UncertainModel,
                // One at an accuracy of one half — the component that tells
                // us nothing — falling to zero at either certainty.
                1.0 - (2.0 * capability.accuracy - 1.0).abs(),
            ),
            Err(_) => (
                ProbeKind::StaleEstimate,
                1.0 - (sample as f64 / MINIMUM_SAMPLE as f64).min(1.0),
            ),
        };
        subjects.insert(key.to_string(), (kind, uncertainty));
    }
    let mut scored: BTreeMap<String, usize> = BTreeMap::new();
    for score in fill_scores {
        *scored
            .entry(format!("instrument:{}", score.object_id.as_str()))
            .or_insert(0) += 1;
    }
    for (subject, sample) in scored {
        let uncertainty = 1.0 - (sample as f64 / CAPACITY_SAMPLE as f64).min(1.0);
        subjects.insert(subject, (ProbeKind::CapacityAtSize, uncertainty));
    }
    // The boundaries arrive already measured and already keyed by
    // `crate::regime_transition::SUBJECT_PREFIX`, which is what keeps a probe
    // openable and settleable under one string. They are seated here rather
    // than beside the candidates so that `settle_due` can read them too: a
    // subject present at zero is measurable and never selected, which is the
    // distinction that lets a regime-boundary probe close with an observation
    // instead of being abandoned as "no longer measured".
    for (subject, uncertainty) in regime_boundaries {
        subjects.insert(subject.clone(), (ProbeKind::RegimeBoundary, *uncertainty));
    }
    subjects
}

/// Close every probe whose question has run out of time.
///
/// Settled where the subject can still be measured — as `Observed`, because
/// nothing took the probe up — and abandoned where it cannot, because a
/// subject that vanished did not resolve to zero gain and recording it as a
/// settlement would put a measurement in the record where there was none.
fn settle_due(
    desk: &mut ExplorationDesk,
    metrics: &Metrics,
    uncertainties: &BTreeMap<String, (ProbeKind, f64)>,
    now: Timestamp,
    notes: &mut Vec<String>,
) {
    for probe in desk.book.due(now) {
        match uncertainties.get(&probe.subject) {
            Some((_, uncertainty_now)) => {
                // Nothing was spent, and zero is recorded as zero: billing a
                // probe for its bound because its bound is the number to hand
                // is how an exploration cost becomes a figure nobody can
                // check.
                let outcome = match ProbeOutcome::new(
                    ProbeEvidence::Observed,
                    Decimal::ZERO,
                    *uncertainty_now,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        notes.push(format!(
                            "{} could not be settled: {}",
                            probe.subject,
                            error.message()
                        ));
                        continue;
                    }
                };
                match desk.book.settle(&probe.id, &outcome) {
                    Ok(settlement) => metrics.count(
                        names::EXPLORATION_PROBES_SETTLED,
                        labels([("evidence", settlement.evidence.as_str())]),
                    ),
                    Err(error) => notes.push(format!(
                        "{} could not be settled: {}",
                        probe.subject,
                        error.message()
                    )),
                }
            }
            None => {
                if let Err(error) = desk
                    .book
                    .abandon(&probe.id, "the subject is no longer measured")
                {
                    notes.push(format!(
                        "{} could not be abandoned: {}",
                        probe.subject,
                        error.message()
                    ));
                }
            }
        }
    }
}

/// The budget the mandates set aside, against the book as it stands.
///
/// The share is the mandate's and the capital is the live equity, so a
/// drawdown shrinks what is explored with. Where users are enrolled the sum
/// of their own shares is the budget — the desk's mandate is the *ceiling*
/// they were admitted under, and adding it to theirs would count the same
/// capital twice. That the sum cannot exceed the ceiling follows from the
/// registry's own admission rules; it is checked here anyway, and refused
/// rather than trimmed, because an invariant that holds by argument
/// elsewhere is exactly the one that stops holding.
fn mandate_budget(ledger: &UserLedger, equity: Decimal) -> Result<Decimal> {
    let desk = ledger.desk();
    let Some(desk_mandate) = ledger.mandate(desk) else {
        return Err(Error::invalid(
            "the ledger holds no desk mandate, so no exploration ceiling can be read",
        ));
    };
    let ceiling = budget_for(equity, desk_mandate.exploration_share())?;
    let mut users = Decimal::ZERO;
    let mut enrolled = 0usize;
    for (user, mandate) in ledger.mandates() {
        if user == desk {
            continue;
        }
        enrolled += 1;
        users = users
            .checked_add(budget_for(
                mandate.investable(),
                mandate.exploration_share(),
            )?)
            .ok_or_else(|| {
                Error::numeric("the enrolled exploration budgets overflow the decimal range")
            })?;
    }
    if enrolled == 0 {
        return Ok(ceiling);
    }
    if users > ceiling {
        return Err(Error::denied(format!(
            "the enrolled mandates set aside {users} for exploration against a desk ceiling of \
             {ceiling}; re-enrol under shares the desk's own mandate covers"
        )));
    }
    Ok(users)
}

/// Give back the hold, if one was taken. Quiet where none was.
///
/// A hold the ledger no longer knows is not an error: the reservation's own
/// expiry returned the capital, which is the backstop for a process that
/// stopped planning, and reporting that as a failure every pass afterwards
/// would bury the refusals that are real.
fn release(
    desk: &mut ExplorationDesk,
    reservations: &mut ReservationLedger,
    now: Timestamp,
    notes: &mut Vec<String>,
) {
    if !desk.held.is_positive() {
        return;
    }
    if reservations.reservation(HOLD_ID).is_some()
        && let Err(error) = reservations.release(HOLD_ID, now)
    {
        notes.push(format!(
            "the exploration hold could not be released: {}",
            error.message()
        ));
    }
    desk.held = Decimal::ZERO;
}

/// Release the standing hold and take the budget again against the book as it
/// now stands, returning what is actually held.
///
/// What is returned is what the ledger *gave*, never what the mandate asked
/// for: planning against a budget the ledger refused would be billing what
/// was planned rather than what ran, and the probes would be bounded by
/// capital nobody is holding.
fn rehold(
    desk: &mut ExplorationDesk,
    reservations: &mut ReservationLedger,
    budget: Decimal,
    now: Timestamp,
    notes: &mut Vec<String>,
) -> Decimal {
    release(desk, reservations, now, notes);
    if !budget.is_positive() {
        return Decimal::ZERO;
    }
    // The committed part of the budget is already inside it: the reservation
    // holds the budget, and the book splits that budget across probes. One
    // hold, one claim about one balance.
    match reservations.reserve(HOLD_ID, budget, now, PROBE_VALIDITY) {
        Ok(()) => {
            desk.held = budget;
            budget
        }
        Err(error) => {
            notes.push(format!(
                "the exploration budget of {budget} could not be held: {}",
                error.message()
            ));
            desk.held = Decimal::ZERO;
            Decimal::ZERO
        }
    }
}

/// Turn the measured uncertainties into candidates the crate can rank.
///
/// Every probe is bounded at the same fraction of the budget — one
/// [`MAXIMUM_OPEN_PROBES`]th, so a full slate of probes fits inside the
/// budget with nothing to spare and no probe reaches the crate's own
/// per-probe ceiling. A budget too small to bound a probe produces no
/// candidates and says so, rather than a probe bounded at zero.
fn candidates_from(
    uncertainties: &BTreeMap<String, (ProbeKind, f64)>,
    budget: Decimal,
    equity: Decimal,
    notes: &mut Vec<String>,
) -> Vec<ProbeCandidate> {
    if !budget.is_positive() {
        return Vec::new();
    }
    let slots = Decimal::from_int(MAXIMUM_OPEN_PROBES as i64);
    let Some(maximum_loss) = budget.checked_div(slots) else {
        notes.push(format!(
            "a budget of {budget} cannot be divided into {MAXIMUM_OPEN_PROBES} probe bounds"
        ));
        return Vec::new();
    };
    if !maximum_loss.is_positive() {
        notes.push(format!(
            "the budget of {budget} is too small to bound a probe; nothing is probed rather \
             than something being probed with no bound"
        ));
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for (subject, (kind, uncertainty)) in uncertainties {
        if *uncertainty <= 0.0 {
            continue;
        }
        // The value of an answer, as one stated figure per unit of
        // uncertainty — see RESOLUTION_VALUE_BPS. Money, so it stays Decimal
        // all the way into the candidate.
        let Some(resolution_value) = equity.checked_apply_bps(RESOLUTION_VALUE_BPS * uncertainty)
        else {
            notes.push(format!(
                "{subject} could not be valued against a book of {equity}; it is not a candidate"
            ));
            continue;
        };
        match ProbeCandidate::new(*kind, subject, *uncertainty, resolution_value, maximum_loss) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => notes.push(format!(
                "{subject} is not a probe candidate: {}",
                error.message()
            )),
        }
    }
    candidates
}

/// Count what the plan selected, by kind.
fn record_selected(metrics: &Metrics, plan: &ExplorationPlan) {
    for probe in &plan.selected {
        metrics.count(
            names::EXPLORATION_PROBES_SELECTED,
            labels([("kind", probe.kind.as_str())]),
        );
    }
}

/// The three account gauges, written on every pass including the idle one.
///
/// Money → `f64` happens here and only here on this path: a gauge is a
/// reading, and the arithmetic above it is all [`Decimal`].
fn record_account(metrics: &Metrics, desk: &ExplorationDesk, held: Decimal) {
    metrics.gauge(names::EXPLORATION_BUDGET, labels([]), held.to_f64());
    metrics.gauge(
        names::EXPLORATION_COMMITTED,
        labels([]),
        desk.book.committed().to_f64(),
    );
    metrics.gauge(
        names::EXPLORATION_SPEND,
        labels([]),
        desk.book.spend().to_f64(),
    );
}

/// The notes, as a clause the cycle report carries. Named rather than
/// counted: a refusal an operator cannot read is a refusal they cannot act on.
fn tail(notes: &[String]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    format!("; {}", notes.join("; "))
}
