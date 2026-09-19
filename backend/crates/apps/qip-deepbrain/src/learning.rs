//! Where the research node actually learns, and where its models age out.
//!
//! The third gap of exactly the same shape as the foundry's and the succession
//! desk's, and the widest of the three. `qip-training` fits models,
//! `qip_kernel::central::models::register_fit` writes the fit's own holdout
//! verdict onto a model card, `ModelRegistry` governs which cards may inform a
//! decision, and `ModelCard::decision_eligibility` refuses a card whose drift
//! score has passed its threshold.
//!
//! Every one of those worked. No running process built a [`ModelRegistry`],
//! called `register_fit`, or fitted anything at all: the whole model-governance
//! subsystem was reachable only from its own tests. `drift_score` was therefore
//! `0.0` on every card that could exist, and the drift branch of
//! `decision_eligibility` could not fire — the shape this codebase keeps
//! producing, where a control is present, correct, and connected to a value
//! nothing writes.
//!
//! # What a round does
//!
//! 1. **Assemble.** Features from the subject's own observed bars, target the
//!    next bar's return. Every feature at bar *i* reads bars up to and
//!    including *i*, and the target spans *i* to *i+1*, so the label is always
//!    on the far side of everything used to predict it.
//! 2. **Fit and register.** `register_fit` takes no `passed` argument: the
//!    verdict is the fit's own out-of-sample result against the skill policy.
//!    A model with no signal is registered as having none rather than not
//!    registered — a failed fit is a fact worth keeping, and deleting it would
//!    let the node retry until something passed with the failures invisible.
//! 3. **Watch it age.** The feature sample the model was fitted on is kept as
//!    its reference. Each later round compares the current window against it
//!    with [`DriftReport::compare`] and records the largest population
//!    stability index across features.
//! 4. **Watch the features age, which is not the same question.** Step 3
//!    asks how far each model is from its own training sample. This asks
//!    whether a *feature* has moved further than the estimators watching it
//!    can explain, and then marks every card naming that feature —
//!    `qip_training::estimators::degraded_models`, §21.1's "one that drifts
//!    past its bound marks every model depending on it as degraded". The two
//!    disagree in both directions: a model's maximum across five features
//!    hides a sixth that moved less than the worst but still past the
//!    estimators' floor, and a card the desk kept no sample for is invisible
//!    to step 3 entirely. The larger of the two readings is what
//!    `record_drift` is given, never the later of the two — a wire that could
//!    lower a drift score is a control rewired to fire less.
//! 5. **Distil.** `qip_training::distill::distil` had the same shape of gap:
//!    fully implemented, well tested, and reachable only from its own crate's
//!    tests -- nothing ever turned a registered teacher into the
//!    [`qip_strategy::model::DistilledModel`] the execution path is actually
//!    allowed to run. Every round that registers a teacher now distils it on
//!    the same holdout tail the fit's own diagnostics were scored against,
//!    and the result -- or, when the probe set could not support a fit, the
//!    reason -- is held on [`LearningRound::distillation`]. This module still
//!    does not promote a distillate any more than it promotes a teacher: the
//!    student is a fact this round produced, for whatever governs
//!    `qip-contracts::policy::PendingPolicy::trained_models` to act on.
//!
//! # Why the maximum across features and not the mean
//!
//! A model reading eight features of which one has moved to a distribution it
//! never saw is a model making predictions from a value outside its training
//! range. Averaging that against seven stable features reports a calm number
//! for a model that is extrapolating, and extrapolation is where a fitted
//! function's error stops being bounded by anything it was measured on.
//!
//! # Which model class, in which regime
//!
//! Blueprint §5.4's meta-learning domain asks "which model class works in
//! which regime". Until 2026-09-19 every round fitted one class — ridge
//! regression — so the question had one possible answer and nothing to
//! learn. A round now fits **two** teachers on the same dataset and the same
//! holdout tail: the linear baseline, always (ADR 0006's classical baseline
//! and ADR 0083's "a learned model against its linear baseline"), and the
//! boosted-stumps challenger. Each class's out-of-sample verdict is scored
//! on a [`Scoreboard`] keyed by class and by the regime the platform itself
//! classified (`Platform::regime_context`) — the same board type §15.1's
//! claim scoreboard and the succession desk use, so a fourth notion of
//! "score by regime" is not invented here.
//!
//! The board is read *before* this round's outcomes are recorded, and it
//! decides one thing: which of the two teachers is registered and distilled.
//! The rule is fail-closed in the direction of the baseline. The challenger
//! is registered only where its precedent in this regime is *established*
//! (the board's own evidence band, not a count chosen here) and its shrunk
//! score is strictly above the baseline's in the same regime. No precedent,
//! an unproven one, or a tie all register the baseline — the class a person
//! can read the coefficients of. The board is bounded by construction: two
//! classes by the product of two regime enums.
//!
//! # What it does not do
//!
//! It does not promote. A registered card enters at development stage and
//! moving it to one that permits decisions stays a governed act elsewhere.
//! This module only ensures that when that decision is taken, the evidence on
//! the card is true and its drift score is a measurement rather than a zero
//! nobody ever wrote.
//!
//! It does not transfer across venues or asset classes. The board is keyed by
//! regime and by nothing else; a precedent earned on one instrument is read
//! for the next only because the regime key is the platform's, not the
//! instrument's, and that is the whole of what this module claims.

use qip_ai::evaluation::DriftReport;
use qip_ai::registry::{ModelCard, ModelRegistry};
use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
use qip_evolution::scoring::{Outcome, Scoreboard};
use qip_kernel::central::models::{ModelRegistration, register_fit};
use qip_market::bar::Bar;
use qip_quant::signal::Horizon;
use qip_training::dataset::TrainingDataset;
use qip_training::distill::{Distillation, FidelityPolicy, StudentForm, distil};
use qip_training::estimators::{DRIFT_BUCKETS, FeatureEstimators, StreamingDrift, degraded_models};
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy};
use std::collections::{BTreeMap, BTreeSet};

/// The features a bar-derived model reads, in the order the dataset carries
/// them.
///
/// Deliberately the vocabulary the strategy harness already computes rather
/// than a second one: two definitions of "momentum over five bars" that drift
/// apart is a defect nobody finds, because both look right in isolation.
const FEATURES: [&str; 5] = [
    "return_1",
    "momentum_5",
    "volatility_10",
    "range_frac",
    "volume_share",
];

/// Bars of history a feature row needs behind it.
///
/// The longest window any feature above reads. A row assembled with less is not
/// a row with a smaller window; it is a row whose features are computed from
/// data that is not there.
const LOOKBACK: usize = 10;

/// The prefix every dataset this desk fits under carries, ahead of the
/// subject's own identifier.
///
/// Load-bearing rather than cosmetic: `register_fit` copies the dataset name
/// onto the card, and that copy is the only record of *which instrument* a
/// standing model was fitted on that outlives this process's own bookkeeping.
/// Changing the shape here without changing [`card_subject`] would leave every
/// card unattributable, and an unattributable card is one this desk declines to
/// measure.
const DATASET_PREFIX: &str = "bars-";

/// The class every round fits and registers unless the board says otherwise.
///
/// Ridge regression: the class whose coefficients a person can read, and the
/// classical baseline ADR 0006 wants computed every time a learned model is.
const BASELINE_CLASS: ModelFamily = ModelFamily::Linear { ridge: 1e-3 };

/// The class every round fits beside the baseline and registers only on an
/// established precedent in the current regime.
const CHALLENGER_CLASS: ModelFamily = ModelFamily::boosted();

/// Why a round registered the class it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassReason {
    /// The board holds no *established* score for the challenger in this
    /// regime — nothing seen, or too little to act on. The baseline is
    /// registered, which is where a desk with no evidence should be.
    NoEstablishedPrecedent,
    /// The challenger's precedent is established and its score is not above
    /// the baseline's in the same regime. A tie keeps the baseline: the
    /// readable class wins unless the other one has actually done better.
    PrecedentBelowBaseline,
    /// The challenger's precedent is established and above the baseline's.
    PrecedentPrefersChallenger,
}

impl ClassReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoEstablishedPrecedent => "no established precedent",
            Self::PrecedentBelowBaseline => "precedent not above the baseline",
            Self::PrecedentPrefersChallenger => "precedent prefers the challenger",
        }
    }
}

/// Which teacher class a round registered, and what the board was told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassChoice {
    /// The regime key the precedent was read under —
    /// `Platform::regime_context`'s `market/volatility` pair.
    pub regime: String,
    /// The family key of the teacher that was registered and distilled.
    pub registered: &'static str,
    pub reason: ClassReason,
    /// Whether the baseline cleared the skill bar this round: the outcome
    /// the board was given for it.
    pub baseline_skilled: bool,
    /// The same for the challenger. `None` where the challenger could not be
    /// fitted at all; a fit the trainer refused is not an observation of
    /// failure and is not scored as one.
    pub challenger_skilled: Option<bool>,
}

impl ClassChoice {
    pub fn describe(&self) -> String {
        let challenger = match self.challenger_skilled {
            Some(true) => "cleared".to_string(),
            Some(false) => "missed".to_string(),
            None => "was not fitted".to_string(),
        };
        format!(
            "registered {} in {} ({}); baseline {} the bar, challenger {}",
            self.registered,
            self.regime,
            self.reason.as_str(),
            if self.baseline_skilled {
                "cleared"
            } else {
                "missed"
            },
            challenger
        )
    }
}

/// Which class to register in `regime`, read from the board before this
/// round's own outcomes join it.
///
/// Fail-closed toward the baseline three ways: no score for the challenger,
/// a score below the board's established band, or a score not strictly
/// above the baseline's all return the baseline. The baseline's own score is
/// required rather than defaulted to the prior, because a comparison with
/// only one side is not a comparison — and since both classes are fitted on
/// every round, a challenger with a score and a baseline without one is a
/// state production cannot reach.
fn choose_class(board: &Scoreboard, regime: &str) -> (ModelFamily, ClassReason) {
    let Some(challenger) = board.score(CHALLENGER_CLASS.as_str(), regime) else {
        return (BASELINE_CLASS, ClassReason::NoEstablishedPrecedent);
    };
    if !challenger.is_confident() {
        return (BASELINE_CLASS, ClassReason::NoEstablishedPrecedent);
    }
    let Some(baseline) = board.score(BASELINE_CLASS.as_str(), regime) else {
        return (BASELINE_CLASS, ClassReason::NoEstablishedPrecedent);
    };
    if challenger.score() > baseline.score() {
        (CHALLENGER_CLASS, ClassReason::PrecedentPrefersChallenger)
    } else {
        (BASELINE_CLASS, ClassReason::PrecedentBelowBaseline)
    }
}

/// The dataset name a round on `subject` fits under.
fn dataset_name(subject: &ObjectId) -> String {
    format!("{DATASET_PREFIX}{}", subject.as_str())
}

/// The instrument a card was fitted on, as the card itself records it.
///
/// `None` when the card names no bar dataset at all, and `None` again when it
/// names two different subjects — refusing rather than picking the first,
/// because a model fitted across two instruments has no single window its
/// drift could honestly be measured against, and guessing one is precisely the
/// failure this function exists to stop.
fn card_subject(card: &ModelCard) -> Option<&str> {
    let mut found: Option<&str> = None;
    for dataset in &card.training_datasets {
        let Some(subject) = dataset.strip_prefix(DATASET_PREFIX) else {
            continue;
        };
        match found {
            None => found = Some(subject),
            Some(seen) if seen == subject => {}
            Some(_) => return None,
        }
    }
    found
}

/// A standing model's fitted feature sample, and the instrument it is of.
///
/// The subject is held beside the columns rather than derived when needed: the
/// desk compares samples long after the round that produced them, and a
/// comparison that has to re-derive which instrument a sample came from is one
/// that will eventually derive it wrongly. See [`LearningDesk::reference`].
#[derive(Clone, Debug)]
struct FittedSample {
    subject: String,
    columns: BTreeMap<String, Vec<f64>>,
}

/// How the learning loop is tuned.
#[derive(Clone, Debug)]
pub struct LearningConfig {
    /// Fit a model every this many research cycles. Zero disables the loop.
    pub every_cycles: u64,
    /// Bars a subject needs before a fit is worth attempting.
    pub minimum_bars: usize,
    /// Fraction of the tail held out of fitting, by time.
    pub holdout_fraction: f64,
    /// Histogram bins for the population stability index.
    ///
    /// Ten is the convention the index was tabulated against, and the
    /// interpretation bands on [`DriftReport::population_stability_index`]
    /// assume it. Changing it changes what 0.25 means.
    pub drift_bins: usize,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            every_cycles: 8,
            minimum_bars: 256,
            holdout_fraction: 0.25,
            drift_bins: 10,
        }
    }
}

/// What one learning round produced.
#[derive(Clone, Debug)]
pub struct LearningRound {
    pub subject: String,
    /// The model registered this round, where one was.
    pub registration: Option<ModelRegistration>,
    /// Drift measured against each standing model's own reference sample.
    pub drift: Vec<DriftObservation>,
    /// Models degraded because a feature they read has moved further than the
    /// estimators watching it can explain, and which of their features did.
    ///
    /// §21.1's "one that drifts past its bound marks every model depending on
    /// it as degraded", as the join
    /// [`qip_training::estimators::degraded_models`] computes. Distinct from
    /// [`Self::drift`], which is each model against *its own* fitted sample: a
    /// model can be absent from this map and present there, and the reverse —
    /// a card fitted before the desk last took a stream reference is measured
    /// here even though the desk kept no sample for it.
    pub degraded: BTreeMap<String, BTreeSet<String>>,
    /// Standing models this round could measure for drift by no route,
    /// because their card names no instrument it was fitted on.
    ///
    /// Reported rather than swallowed. A card nobody measures keeps the drift
    /// score `ModelCard::decision_eligibility` reads at zero, so it reads as
    /// undrifted forever — the `MaxExpectedShortfall` shape, a control
    /// connected to a value nothing writes. The desk will not guess an
    /// instrument for such a card, because a drift score taken against the
    /// wrong one is worse than none; so it names the card instead, on the
    /// round line an operator reads.
    ///
    /// Distinct from a card fitted on a *different* subject, which is absent
    /// from this list and from [`Self::drift`] alike: that is ordinary
    /// rotation, and that card is measured on the round its own instrument
    /// comes up.
    pub unattributed: Vec<String>,
    /// Models the registry will no longer let inform a decision, with why.
    pub ineligible: Vec<String>,
    /// The student distilled from this round's teacher onto the same holdout
    /// tail the fit's own diagnostics were scored against.
    ///
    /// Held here rather than acted on: promoting a distillate to the model
    /// the execution path actually runs is a decision for whatever governs
    /// `qip-contracts::policy::PendingPolicy`, not for the desk that fits it.
    /// `None` when no teacher was registered this round, or when the probe
    /// set could not support a fit -- see `distillation_refusal` for why.
    pub distillation: Option<Distillation>,
    /// Why `distillation` is `None` despite a teacher having been
    /// registered this round: the specific reason `distil` refused, so a
    /// caller need not guess whether nothing was attempted or something
    /// failed.
    pub distillation_refusal: Option<String>,
    /// The campaign the window was assembled through (§22.4), when the
    /// engine assembled it through one — every production round does; a
    /// desk driven directly by a test may not have.
    pub campaign: Option<crate::campaign::CampaignSummary>,
    /// Why no campaign opened this round: the stream was refused at the
    /// door — neither a source the platform holds an admission for nor one
    /// it generated, or a standing admission that has stopped granting.
    /// Nothing else on this round is set when this is: no fit, no drift, no
    /// campaign. A fact about one subject and one round, on the round line
    /// where an operator reads it, rather than the error that stopped the
    /// node loop until 2026-09-12.
    pub refused_by_door: Option<String>,
    /// Which teacher class this round registered and why — the board's
    /// answer to "which model class works in this regime". `None` on a round
    /// that registered nothing.
    pub class_choice: Option<ClassChoice>,
}

impl LearningRound {
    /// A round that opened no campaign because the door refused the stream.
    pub fn refused_at_door(subject: &ObjectId, reason: impl Into<String>) -> Self {
        Self {
            subject: subject.as_str().to_string(),
            registration: None,
            drift: Vec::new(),
            degraded: BTreeMap::new(),
            unattributed: Vec::new(),
            ineligible: Vec::new(),
            distillation: None,
            distillation_refusal: None,
            campaign: None,
            refused_by_door: Some(reason.into()),
            class_choice: None,
        }
    }

    /// Every feature that degraded at least one model this round.
    ///
    /// The union of [`Self::degraded`]'s values rather than a second field:
    /// two claims about which features drifted would disagree eventually, and
    /// the one that reaches an operator would be the wrong one.
    pub fn drifted_features(&self) -> BTreeSet<&str> {
        self.degraded
            .values()
            .flatten()
            .map(String::as_str)
            .collect()
    }

    pub fn describe(&self) -> String {
        if let Some(reason) = &self.refused_by_door {
            return format!(
                "learning: no campaign for {} — refused at the door: {reason}",
                self.subject
            );
        }
        let registered = match &self.registration {
            Some(registration) => registration.summarise(),
            None => "no fit this round".to_string(),
        };
        let class = match &self.class_choice {
            Some(choice) => format!("; {}", choice.describe()),
            None => String::new(),
        };
        let distilled = match &self.distillation {
            Some(distillation) => {
                let approved = distillation.is_promotable(&FidelityPolicy::default());
                format!(
                    "; distilled a {} student whose fidelity is {} the default policy",
                    distillation.form().as_str(),
                    if approved { "within" } else { "short of" }
                )
            }
            None => match &self.distillation_refusal {
                Some(reason) => format!("; no student distilled: {reason}"),
                None => String::new(),
            },
        };
        let campaign = match &self.campaign {
            Some(campaign) => format!("; {}", campaign.describe()),
            None => String::new(),
        };
        let unattributed = if self.unattributed.is_empty() {
            String::new()
        } else {
            format!(
                "; {} model(s) measurable against no instrument",
                self.unattributed.len()
            )
        };
        let degraded = if self.degraded.is_empty() {
            String::new()
        } else {
            format!(
                "; {} model(s) degraded by {} drifted feature(s)",
                self.degraded.len(),
                self.drifted_features().len()
            )
        };
        format!(
            "learning: {registered}{class}; {} model(s) measured for drift, {} \
             ineligible{unattributed}{degraded}{distilled}{campaign}",
            self.drift.len(),
            self.ineligible.len()
        )
    }
}

/// One model's drift, and the feature that produced it.
#[derive(Clone, Debug)]
pub struct DriftObservation {
    pub reference: String,
    /// The largest population stability index across the model's features.
    pub population_stability_index: f64,
    /// The feature that produced it. The number alone tells an operator
    /// something moved; this tells them what to look at.
    pub worst_feature: String,
    /// Whether the index has passed *this card's* drift threshold.
    ///
    /// Deliberately not "is this model still decision-eligible".
    /// `decision_eligibility` refuses a development-stage card before it ever
    /// reads the drift score, so reporting eligibility here would say
    /// "ineligible" for every freshly fitted model and read as though drift had
    /// disqualified it. Conflating a stage with a measurement is how a number
    /// that means nothing ends up on a dashboard meaning something.
    pub above_threshold: bool,
}

/// Running totals for the shutdown report.
#[derive(Clone, Copy, Debug, Default)]
pub struct LearningStats {
    pub rounds: u64,
    pub registered: u64,
    /// Fits that did not clear the skill bar. Kept separately because a node
    /// registering only failures is a different problem from one registering
    /// nothing.
    pub without_skill: u64,
    pub drift_measurements: u64,
    /// Rounds whose campaign was refused at the door and so fitted nothing.
    /// Kept separately from `rounds`, which counts fits attempted: a node
    /// refused every round and a node that never came due read differently.
    pub refused_at_door: u64,
}

/// Fits models from observed bars and watches the ones it has fitted age.
pub struct LearningDesk {
    config: LearningConfig,
    policy: SkillPolicy,
    registry: ModelRegistry,
    /// Per registered model, the feature columns it was fitted on and the
    /// instrument those columns are of.
    ///
    /// Kept because drift is a comparison against *what this model saw*, and
    /// nothing else in the platform remembers that. A drift score computed
    /// against the current window's own earlier half would be measuring the
    /// data's recent stability rather than this model's distance from its
    /// training set.
    ///
    /// The subject is half the key in practice. The deep brain rotates
    /// subjects — `EvolutionEngine::maybe_learn` re-takes the choice every
    /// round from whichever instrument currently carries the greatest notional
    /// depth — and this map was once compared wholesale against whatever
    /// window the round brought. A model fitted on one instrument was then
    /// scored against another's bars, and `ModelCard::decision_eligibility`
    /// refuses on that score: a model could be ruled out of decisions by a
    /// stability index of 12.4 that was about a different instrument. That is
    /// worse than no score, because it reads as a measurement.
    reference: BTreeMap<String, FittedSample>,
    /// The feature stream as it stood when the desk last fitted, held as
    /// summaries rather than as the stream.
    ///
    /// The reference the *stream-level* drift is taken against, and
    /// deliberately not the same thing as `reference` above. That map answers
    /// "how far is this model from the sample it was fitted on", which is a
    /// question per model and needs the model's own rows. This answers "has
    /// this feature moved further than the estimators watching it can
    /// explain", which is a question about the feature and has one answer for
    /// every model that reads it — which is what makes
    /// [`degraded_models`]' join sound. A drifted set derived per model could
    /// not be joined onto another model's card without asserting that one
    /// model's distance from its own training window says something about a
    /// second model's, which it does not.
    ///
    /// Refreshed on every successful fit, so the comparison is always against
    /// the most recent distribution the desk trained on. A card fitted before
    /// that is compared against a reference newer than its own, which can
    /// only mark it degraded where its own window would not have — the
    /// direction a control is allowed to be wrong in, and it is re-measured
    /// from scratch every round rather than latched.
    ///
    /// Keyed by subject, and that is the narrowest keying the join stays
    /// sound under. "One answer for every model reading this feature" has to
    /// hold, or `degraded_models` is joining one model's distance from its
    /// own training window onto another's card. It holds *within* a subject:
    /// every model fitted on this instrument reads the same `return_1` and the
    /// question "has it moved" has one answer for all of them. It does not
    /// hold across subjects, because `return_1` on one instrument moving says
    /// nothing about `return_1` on another — so a single global reference made
    /// the join unsound in exactly the way the per-model derivation it was
    /// built to avoid would have.
    stream_reference: BTreeMap<String, FeatureEstimators>,
    /// How often each teacher class cleared the skill bar, by regime — the
    /// §5.4 meta-learning board for model classes. Subjects are the two
    /// family keys, contexts are `Platform::regime_context` pairs, so it is
    /// bounded by construction and does not grow with uptime. Read by
    /// [`choose_class`] before a round's own fits are scored onto it.
    classes: Scoreboard,
    /// Fits attempted, which is what a model's version is drawn from.
    fits: u64,
    seed: u64,
    stats: LearningStats,
}

impl std::fmt::Debug for LearningDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearningDesk")
            .field("config", &self.config)
            .field("models", &self.reference.len())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl LearningDesk {
    pub fn new(config: LearningConfig, seed: u64) -> Self {
        Self {
            config,
            policy: SkillPolicy::default(),
            registry: ModelRegistry::new(),
            reference: BTreeMap::new(),
            stream_reference: BTreeMap::new(),
            classes: Scoreboard::models(),
            fits: 0,
            seed,
            stats: LearningStats::default(),
        }
    }

    /// The model-class board: how often each teacher class cleared the skill
    /// bar, by regime. For an operator reading why a round registered the
    /// class it did; nothing outside this desk writes to it.
    pub const fn class_board(&self) -> &Scoreboard {
        &self.classes
    }

    /// Seed the class board with an outcome a real fit would have produced.
    /// Test-only: the one production writer is a round's own fit, and a
    /// public writer here would let anything forge the precedent the
    /// registration decision reads.
    #[cfg(test)]
    fn observe_class(&mut self, family: &ModelFamily, regime: &str, skilled: bool) {
        self.classes
            .observe(Outcome::binary(family.as_str(), regime, skilled));
    }

    pub const fn stats(&self) -> LearningStats {
        self.stats
    }

    pub const fn enabled(&self) -> bool {
        self.config.every_cycles > 0
    }

    pub const fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    /// The registry, mutably, for the governed acts this module does not
    /// perform itself -- promotion above all. Kept separate from `registry`
    /// so that "this desk fitted a model" and "somebody staged it for
    /// decisions" stay two different sentences in the call graph.
    pub const fn registry_mut(&mut self) -> &mut ModelRegistry {
        &mut self.registry
    }

    /// Models this desk has fitted and still holds a reference sample for.
    pub fn tracked(&self) -> usize {
        self.reference.len()
    }

    /// Fit a model if the cadence says so, then measure every standing model
    /// against the window that has arrived since it was fitted.
    ///
    /// `None` means "not this cycle", which is the normal case.
    ///
    /// `regime` is the key the platform classified for this subject
    /// (`Platform::regime_context`), read by the caller and handed in rather
    /// than recomputed here: a desk that classified its own regime would be
    /// a second answer to a question the platform has already answered.
    pub fn maybe_learn(
        &mut self,
        subject: &ObjectId,
        bars: &[Bar],
        regime: &str,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<LearningRound>> {
        if !self.due(cycle) || bars.len() < self.config.minimum_bars {
            return Ok(None);
        }
        Ok(Some(self.learn(subject, bars, regime, now)?))
    }

    /// Whether the cadence says a round runs this cycle. Split from
    /// [`Self::maybe_learn`] so the engine can assemble the window through a
    /// campaign only on the cycles a fit will actually use it.
    pub const fn due(&self, cycle: u64) -> bool {
        self.config.every_cycles != 0 && cycle.is_multiple_of(self.config.every_cycles)
    }

    /// Bars a subject needs before a fit is worth attempting.
    pub const fn minimum_bars(&self) -> usize {
        self.config.minimum_bars
    }

    /// Note a round that opened no campaign because the door refused it.
    pub const fn note_door_refusal(&mut self) {
        self.stats.refused_at_door = self.stats.refused_at_door.saturating_add(1);
    }

    /// Fit on a window the caller has already assembled — through a
    /// campaign, in production — refusing one shorter than the minimum
    /// rather than fitting on it.
    pub fn learn_window(
        &mut self,
        subject: &ObjectId,
        bars: &[Bar],
        regime: &str,
        now: Timestamp,
    ) -> Result<LearningRound> {
        if bars.len() < self.config.minimum_bars {
            return Err(Error::invalid(format!(
                "a window of {} bar(s) is below the {} the desk fits on; the campaign that \
                 assembled it should not have",
                bars.len(),
                self.config.minimum_bars
            )));
        }
        self.learn(subject, bars, regime, now)
    }

    fn learn(
        &mut self,
        subject: &ObjectId,
        bars: &[Bar],
        regime: &str,
        now: Timestamp,
    ) -> Result<LearningRound> {
        if regime.trim().is_empty() {
            return Err(Error::invalid(
                "a learning round needs the regime the platform classified for its subject; \
                 an empty key would score every class under one context and the board would \
                 learn which class works in no regime at all — pass `Platform::regime_context`",
            ));
        }
        self.stats.rounds += 1;
        let columns = feature_columns(bars);
        // Every comparison below is scoped to this instrument. A model fitted
        // on another one is left alone this round, not measured against these
        // bars; it is measured when its own subject next comes round.
        let on_subject = subject.as_str();

        // Drift first, against the window as it stands *before* this round's
        // fit joins the registry. Measuring a model against a window that
        // includes the data a newer model was fitted on would be comparing two
        // different questions.
        //
        // Two independent readings feed one score per model, and they are
        // merged by `max` rather than by whichever ran last. A model's own
        // comparison against the sample it was fitted on can be calm while a
        // feature it reads has moved past what the estimators watching it can
        // explain — the maximum across a model's features hides a feature that
        // moved less than another one did. Letting the second reading *lower*
        // the first would be the `MaxExpectedShortfall` failure inverted: not
        // a control that cannot fire, but one rewired to fire less than it did
        // before the wire was added.
        let (stream, current) = self.stream_drift(on_subject, &columns)?;
        let drifted: BTreeSet<String> = stream.keys().cloned().collect();
        // Collected before the registry is borrowed mutably below, and owned
        // because `record_drift` needs the mutable borrow while the join's
        // result is still being read.
        //
        // Only cards this instrument's features can speak for. A card the desk
        // kept no sample for is still here — that is the whole point of the
        // feature-level join — provided the card itself says it was fitted on
        // this subject. One that says nothing is carried out as `unattributed`
        // rather than measured or silently dropped.
        let mut unattributed: Vec<String> = Vec::new();
        let mut dependencies: Vec<(String, Vec<String>)> = Vec::new();
        for card in self.registry.iter() {
            match card_subject(card) {
                Some(fitted_on) if fitted_on == on_subject => {
                    dependencies.push((card.reference(), card.features.clone()));
                }
                Some(_) => {}
                None => unattributed.push(card.reference()),
            }
        }
        let degraded = degraded_models(
            &drifted,
            dependencies
                .iter()
                .map(|(reference, features)| (reference.as_str(), features.as_slice())),
        );

        let mut measured: BTreeMap<String, (f64, String)> = BTreeMap::new();
        for (reference, sample) in &self.reference {
            if sample.subject != on_subject {
                continue;
            }
            if let Some((worst_feature, index)) =
                worst_drift(&sample.columns, &columns, self.config.drift_bins)
            {
                measured.insert(reference.clone(), (index, worst_feature));
            }
        }
        for (reference, features) in &degraded {
            for feature in features {
                let Some(observed) = stream.get(feature) else {
                    continue;
                };
                let index = observed.population_stability_index;
                let entry = measured
                    .entry(reference.clone())
                    .or_insert_with(|| (index, feature.clone()));
                if index > entry.0 {
                    *entry = (index, feature.clone());
                }
            }
        }

        let mut drift = Vec::new();
        for (reference, (index, worst_feature)) in measured {
            self.registry.record_drift(&reference, index)?;
            let above_threshold = self
                .registry
                .get(&reference)
                .is_some_and(|card| index > card.drift_threshold);
            self.stats.drift_measurements += 1;
            drift.push(DriftObservation {
                reference,
                population_stability_index: index,
                worst_feature,
                above_threshold,
            });
        }

        let (registration, distillation, distillation_refusal, class_choice) =
            match self.fit(subject, bars, &columns, regime, now) {
                Ok((registration, distillation, distillation_refusal, choice)) => {
                    if registration.passed {
                        self.stats.registered += 1;
                    } else {
                        self.stats.without_skill += 1;
                    }
                    self.reference.insert(
                        registration.reference.clone(),
                        FittedSample {
                            subject: on_subject.to_string(),
                            columns,
                        },
                    );
                    // The stream reference moves forward only on a round that
                    // fitted, and only for the subject that round fitted on. A
                    // round that could not fit leaves it where it was, so the
                    // next comparison is still against a distribution some
                    // model was actually trained on rather than against the
                    // last window that happened to arrive — and a round on a
                    // different instrument leaves this one's alone, so
                    // rotation does not silently replace one instrument's
                    // reference distribution with another's.
                    self.stream_reference
                        .insert(on_subject.to_string(), current);
                    (
                        Some(registration),
                        distillation,
                        distillation_refusal,
                        Some(choice),
                    )
                }
                // A round that could not fit is not a round that found
                // nothing: too little history, a degenerate target, a
                // dataset the trainer refused. Returned on the round rather
                // than propagated, so one unfittable subject does not stop
                // the node.
                Err(error) => {
                    return Ok(LearningRound {
                        subject: subject.as_str().to_string(),
                        registration: None,
                        drift,
                        degraded,
                        unattributed,
                        ineligible: vec![error.message().to_string()],
                        distillation: None,
                        distillation_refusal: None,
                        campaign: None,
                        refused_by_door: None,
                        class_choice: None,
                    });
                }
            };

        let ineligible = self
            .registry
            .ineligible(now)
            .into_iter()
            .map(|(card, reason)| format!("{}: {reason}", card.reference()))
            .collect();

        Ok(LearningRound {
            subject: subject.as_str().to_string(),
            registration,
            drift,
            degraded,
            unattributed,
            ineligible,
            distillation,
            distillation_refusal,
            campaign: None,
            refused_by_door: None,
            class_choice,
        })
    }

    /// The features whose distribution has moved further than the estimators
    /// watching them can explain, and the summaries this round produced.
    ///
    /// §21.1's streaming half, as the thing it is for. The four estimators
    /// each declare their own error;
    /// [`StreamingDrift::is_material`] compares the index against the floor
    /// those errors imply, so a feature is "drifted" only when the shift is
    /// larger than two digests of one distribution could manufacture between
    /// them. A fixed threshold here would be comparing signal plus estimator
    /// noise against a number chosen without knowing how much noise there was.
    ///
    /// Empty on this subject's first round, and after any round on this
    /// subject that could not fit, because there is no reference for *this
    /// instrument* to compare against. That is reported as "nothing measured"
    /// by there being no entry, not as "nothing moved" by an index of zero —
    /// and, since the reference is per subject, never as a shift measured
    /// against a distribution belonging to some other instrument.
    ///
    /// The returned summaries are the caller's to install as the next
    /// reference, and are deliberately *not* installed here: a round that
    /// fails to fit must not move the reference, and a function that both
    /// measures and advances the thing it measures against cannot be asked
    /// for one without the other.
    fn stream_drift(
        &mut self,
        on_subject: &str,
        columns: &BTreeMap<String, Vec<f64>>,
    ) -> Result<(BTreeMap<String, StreamingDrift>, FeatureEstimators)> {
        let seed = self.seed;
        let mut current = FeatureEstimators::standard(seed)?;
        for (name, values) in columns {
            for value in values {
                current.observe(name, *value)?;
            }
        }
        let material = match self.stream_reference.get_mut(on_subject) {
            None => BTreeMap::new(),
            Some(reference) => current
                .drift_against(reference, DRIFT_BUCKETS)?
                .into_iter()
                .filter(|(_, drift)| drift.is_material())
                .collect(),
        };
        Ok((material, current))
    }

    fn fit(
        &mut self,
        subject: &ObjectId,
        bars: &[Bar],
        columns: &BTreeMap<String, Vec<f64>>,
        regime: &str,
        now: Timestamp,
    ) -> Result<(
        ModelRegistration,
        Option<Distillation>,
        Option<String>,
        ClassChoice,
    )> {
        let targets = next_bar_returns(bars);
        let times: Vec<Timestamp> = bars
            .iter()
            .skip(LOOKBACK)
            .take(targets.len())
            .map(Bar::close_time)
            .collect();
        let names: Vec<String> = FEATURES.iter().map(|name| (*name).to_string()).collect();
        let rows: Vec<Vec<f64>> = (0..targets.len())
            .map(|row| {
                names
                    .iter()
                    .map(|name| {
                        columns
                            .get(name)
                            .and_then(|c| c.get(row))
                            .copied()
                            .unwrap_or(0.0)
                    })
                    .collect()
            })
            .collect();
        if rows.len() != times.len() {
            return Err(Error::invalid(format!(
                "{} feature row(s) against {} timestamp(s); a row whose instant is unknown \
                 cannot be split by time, and a split that is not by time leaks the future",
                rows.len(),
                times.len()
            )));
        }

        let dataset = TrainingDataset::new(dataset_name(subject), names, rows, targets, times)?;
        // Versioned by the desk's own fit count, not by the observation
        // count. `ModelRegistry::register` replaces a card of the same
        // reference outright, and two rounds over the same amount of history
        // produced the same version -- so the second fit silently overwrote the
        // first, taking its drift score back to zero with it. A model whose
        // measured drift is erased by the arrival of its successor is a control
        // that resets itself exactly when it matters.
        self.fits += 1;
        // The precedent is read before this round's fits are scored onto the
        // board, so a round cannot vote for itself: the class registered now
        // is decided by what earlier rounds in this regime showed.
        let (chosen, reason) = choose_class(&self.classes, regime);
        // The class is in the model's name, so the registry reference a
        // strategy candidate carries says which function it is. Nothing
        // else on the card records the class: `register_fit` copies the
        // features, the dataset and the holdout figures, and a card that
        // could be a linear fit or a sixty-stump ensemble without saying
        // which is a card a reviewer cannot read.
        let spec_for = |family: ModelFamily| {
            TrainingSpec::new(
                format!("bar-{}-{}", family.as_str(), subject.as_str()),
                format!("0.{}.0", self.fits),
                "central-research",
                dataset.name(),
                family,
            )
            .with_purpose(format!(
                "predicts the next bar's return on {} from its own observed bars",
                subject.as_str()
            ))
            .with_horizon(Horizon::Intraday)
            .with_holdout(self.config.holdout_fraction)
            .with_seed(self.seed)
        };
        let spec = spec_for(chosen);

        // Both classes are fitted on the same dataset and scored on the same
        // holdout tail, whichever one is registered. The baseline's fit is
        // the round's — a baseline the trainer refuses is a round that could
        // not fit. The challenger's is not: a refused challenger is reported
        // on the choice and given no outcome on the board, because a fit
        // that never happened is not evidence that the class fails here.
        let trainer = LocalTrainer::new();
        let baseline = trainer.fit(&spec_for(BASELINE_CLASS), &dataset, now)?;
        let challenger = trainer.fit(&spec_for(CHALLENGER_CLASS), &dataset, now);
        let baseline_skilled = baseline
            .fit()
            .is_some_and(|fit| fit.claims_skill(&self.policy));
        let challenger_skilled = challenger.as_ref().ok().map(|teacher| {
            teacher
                .fit()
                .is_some_and(|fit| fit.claims_skill(&self.policy))
        });
        self.classes.observe(Outcome::binary(
            BASELINE_CLASS.as_str(),
            regime,
            baseline_skilled,
        ));
        if let Some(skilled) = challenger_skilled {
            self.classes
                .observe(Outcome::binary(CHALLENGER_CLASS.as_str(), regime, skilled));
        }
        let teacher = match chosen {
            ModelFamily::Linear { .. } => baseline,
            ModelFamily::BoostedStumps { .. } => challenger?,
        };
        let choice = ClassChoice {
            regime: regime.to_string(),
            registered: chosen.as_str(),
            reason,
            baseline_skilled,
            challenger_skilled,
        };
        let registration = register_fit(
            &mut self.registry,
            &teacher,
            &self.policy,
            "central-research",
            now,
        )?;

        // Distil the teacher into the linear form the execution path is
        // actually allowed to run, probed on the same holdout tail the
        // fit's own diagnostics were scored against. Reusing the training
        // rows as a probe would measure how well the student memorised the
        // teacher's training set, not how well it tracks the teacher's
        // actual behaviour -- the same reason the fit itself is scored on a
        // holdout rather than in sample.
        let (distillation, distillation_refusal) =
            match dataset.split_at_fraction(spec.holdout_fraction) {
                Ok((_, probe)) => {
                    match distil(&teacher, &probe, StudentForm::Linear { ridge: 1e-3 }, 0.0) {
                        Ok(distillation) => (Some(distillation), None),
                        Err(error) => (None, Some(error.message().to_string())),
                    }
                }
                Err(error) => (None, Some(error.message().to_string())),
            };

        Ok((registration, distillation, distillation_refusal, choice))
    }
}

/// The largest population stability index across the features two samples
/// share, and the feature that produced it.
///
/// The maximum and not the mean: a model reading eight features of which one
/// has moved outside its training range is extrapolating, and averaging that
/// against seven stable features reports a calm number for exactly the case
/// where a fitted function's error stops being bounded by anything measured.
fn worst_drift(
    reference: &BTreeMap<String, Vec<f64>>,
    current: &BTreeMap<String, Vec<f64>>,
    bins: usize,
) -> Option<(String, f64)> {
    reference
        .iter()
        .filter_map(|(name, sample)| {
            let live = current.get(name)?;
            let report = DriftReport::compare(sample, live, bins);
            Some((name.clone(), report.population_stability_index))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
}

/// Feature columns over `bars`, one row per bar that has both a full lookback
/// behind it and a next bar ahead of it.
///
/// Row *i* reads bars up to and including `bars[LOOKBACK + i]`, and the target
/// for that row spans that bar to the next. The label is therefore always on
/// the far side of every value used to predict it, which is the property a
/// backtest cannot recover if the dataset does not have it.
fn feature_columns(bars: &[Bar]) -> BTreeMap<String, Vec<f64>> {
    let mut columns: BTreeMap<String, Vec<f64>> = FEATURES
        .iter()
        .map(|name| ((*name).to_string(), Vec::new()))
        .collect();
    if bars.len() <= LOOKBACK + 1 {
        return columns;
    }
    let closes: Vec<f64> = bars.iter().map(|bar| bar.close.to_f64()).collect();
    let volumes: Vec<f64> = bars.iter().map(|bar| bar.volume.to_f64()).collect();

    let mut push = |name: &str, value: f64| {
        if let Some(column) = columns.get_mut(name) {
            column.push(if value.is_finite() { value } else { 0.0 });
        }
    };

    for at in LOOKBACK..bars.len() - 1 {
        let close = closes[at];
        push("return_1", ratio(close, closes[at - 1]));
        push("momentum_5", ratio(close, closes[at - 5]));

        let window: Vec<f64> = (at - 9..=at)
            .map(|i| ratio(closes[i], closes[i - 1]))
            .collect();
        push("volatility_10", qip_numerics::stats::stddev(&window));

        let bar = &bars[at];
        let high = bar.high.to_f64();
        let low = bar.low.to_f64();
        push(
            "range_frac",
            if close.abs() > f64::EPSILON {
                (high - low) / close
            } else {
                0.0
            },
        );

        // Volume relative to its own trailing mean, not raw volume. A raw
        // level is an instrument-specific magnitude, and a model fitted on one
        // instrument's volume learns that instrument's size rather than
        // anything about markets.
        let trailing: f64 = volumes[at - LOOKBACK..at].iter().sum::<f64>() / LOOKBACK as f64;
        push(
            "volume_share",
            if trailing > f64::EPSILON {
                volumes[at] / trailing
            } else {
                0.0
            },
        );
    }
    columns
}

/// The return from each feature row's bar to the next.
fn next_bar_returns(bars: &[Bar]) -> Vec<f64> {
    if bars.len() <= LOOKBACK + 1 {
        return Vec::new();
    }
    (LOOKBACK..bars.len() - 1)
        .map(|at| ratio(bars[at + 1].close.to_f64(), bars[at].close.to_f64()))
        .collect()
}

/// A simple return, guarded against a zero denominator.
///
/// The crossing point from money to statistics: the closes are `Decimal`
/// because they are prices, and everything from here is `f64` because a return
/// is a ratio and a ratio is not money.
fn ratio(current: f64, previous: f64) -> f64 {
    if previous.abs() < 1e-12 {
        return 0.0;
    }
    current / previous - 1.0
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn subject() -> ObjectId {
        ObjectId::from_string("OBJ0000000000000000000AAA")
    }

    /// A second instrument. The deep brain rotates subjects, so two of them
    /// reaching one desk is the ordinary case rather than a contrived one.
    fn other_subject() -> ObjectId {
        ObjectId::from_string("OBJ0000000000000000000ZZZ")
    }

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    /// The regime key a production round hands in from
    /// `Platform::regime_context`; the pair shape is the platform's.
    const REGIME: &str = "trending/calm";

    fn learning_desk() -> LearningDesk {
        LearningDesk::new(
            LearningConfig {
                every_cycles: 1,
                minimum_bars: 64,
                ..LearningConfig::default()
            },
            7,
        )
    }

    #[test]
    fn a_round_registers_a_model_carrying_its_own_out_of_sample_verdict() -> Result<()> {
        // Nothing in any running process had ever built a registry, fitted a
        // model, or called `register_fit`. The whole model-governance
        // subsystem -- skill verdicts, evaluations, drift, eligibility -- was
        // reachable only from its own tests.
        let mut desk = learning_desk();
        // The premise: nothing is registered before the round.
        assert_eq!(desk.registry().len(), 0);
        assert_eq!(desk.tracked(), 0);

        let bars = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &bars, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;

        let registration = round.registration.ok_or_else(|| {
            Error::not_found(format!("a registration; got {:?}", round.ineligible))
        })?;
        assert_eq!(
            desk.registry().len(),
            1,
            "the registry did not receive the fit"
        );
        assert_eq!(
            desk.tracked(),
            1,
            "no reference sample was kept to measure drift against"
        );

        // The card's evaluation is the fit's own holdout result, not a claim
        // about it: `register_fit` takes no `passed` argument.
        let card = desk
            .registry()
            .get(&registration.reference)
            .ok_or_else(|| Error::not_found("the card just registered"))?;
        let evaluation = card
            .latest_evaluation()
            .ok_or_else(|| Error::not_found("the evaluation on the card"))?;
        assert_eq!(
            evaluation.passed, registration.passed,
            "the card's evaluation disagrees with the verdict the fit produced"
        );
        let holdout_r2 = evaluation
            .metrics
            .get("holdout_r2")
            .copied()
            .ok_or_else(|| Error::not_found("the out-of-sample figure on the evaluation"))?;

        // The verdict is the fit's own out-of-sample result and not a claim
        // about it: `register_fit` takes no `passed` argument. Asserted against
        // the policy's own bar in both directions, because a card that simply
        // agreed with itself would agree just as happily if the verdict were
        // inverted.
        let policy = SkillPolicy::default();
        assert_eq!(
            evaluation.passed,
            holdout_r2 >= policy.minimum_holdout_r2,
            "the card says passed={} on a holdout R-squared of {holdout_r2:.4} against a \
             {:.4} bar",
            evaluation.passed,
            policy.minimum_holdout_r2
        );

        // And the other direction, on a series with nothing to learn. Without
        // this, every assertion above holds for a registry that marks
        // everything as having skill.
        let mut fresh = learning_desk();
        let noise = super::tests_support::unlearnable(400);
        let refused = fresh
            .maybe_learn(&subject(), &noise, REGIME, 1, at())?
            .and_then(|round| round.registration)
            .ok_or_else(|| Error::not_found("a registration for the noise fit"))?;
        assert!(
            !refused.passed,
            "a model fitted on a series with no predictable structure was registered as \
             having cleared the skill bar"
        );
        let noise_card = fresh
            .registry()
            .get(&refused.reference)
            .ok_or_else(|| Error::not_found("the noise card"))?;
        assert!(
            noise_card
                .latest_evaluation()
                .is_some_and(|evaluation| !evaluation.passed),
            "the card disagrees with the registration about whether the fit had skill"
        );
        Ok(())
    }

    #[test]
    fn a_model_whose_features_have_moved_records_drift_past_its_own_threshold() -> Result<()> {
        // The control that could not fire. `decision_eligibility` refuses a
        // card whose drift score has passed its threshold, and `drift_score`
        // was 0.0 on every card that could exist because nothing outside a
        // test ever called `record_drift`.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &calm, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("the first round"))?;
        let reference = round
            .registration
            .ok_or_else(|| Error::not_found("a registration"))?
            .reference;

        // The premise, in two parts: the model starts at zero drift, and the
        // first round measured nothing because there was nothing standing to
        // measure.
        assert!(
            round.drift.is_empty(),
            "the first round measured drift against itself"
        );
        let threshold = desk
            .registry()
            .get(&reference)
            .map(|card| card.drift_threshold)
            .ok_or_else(|| Error::not_found("the card"))?;
        assert_eq!(
            desk.registry().get(&reference).map(|card| card.drift_score),
            Some(0.0),
            "the card began with a drift score somebody had already written"
        );

        // A regime the model has never seen: the same instrument, now moving
        // in violent alternating jumps instead of a steady drift.
        let shocked = super::tests_support::shocked(400);
        let second = desk
            .maybe_learn(&subject(), &shocked, REGIME, 2, at())?
            .ok_or_else(|| Error::not_found("the second round"))?;

        let observation = second
            .drift
            .iter()
            .find(|observation| observation.reference == reference)
            .ok_or_else(|| Error::not_found("drift measured against the standing model"))?;
        assert!(
            observation.population_stability_index > threshold,
            "a regime change produced a stability index of {:.3} against a {:.3} threshold; \
             the comparison is not sensitive enough to be a control",
            observation.population_stability_index,
            threshold
        );
        assert!(observation.above_threshold);
        assert!(
            FEATURES.contains(&observation.worst_feature.as_str()),
            "the drift names a feature the model does not read: {}",
            observation.worst_feature
        );

        // The reported index is the *largest* across features, not an average
        // and not the smallest. A model reading five features of which one has
        // moved outside its training range is extrapolating, and a statistic
        // that lets four calm features speak for the fifth reports a calm
        // number for exactly that case.
        let before = super::feature_columns(&calm);
        let after = super::feature_columns(&shocked);
        let mut indices: Vec<(String, f64)> = before
            .iter()
            .filter_map(|(name, sample)| {
                let live = after.get(name)?;
                Some((
                    name.clone(),
                    DriftReport::compare(sample, live, LearningConfig::default().drift_bins)
                        .population_stability_index,
                ))
            })
            .collect();
        indices.sort_by(|left, right| left.1.total_cmp(&right.1));
        // The premise: the features disagree. If every one moved by the same
        // amount, maximum and minimum would be the same number and this would
        // prove nothing about which was chosen.
        let (lowest, highest) = match indices.as_slice() {
            [first, .., last] => (first, last),
            other => {
                return Err(Error::not_found(format!(
                    "at least two features; got {}",
                    other.len()
                )));
            }
        };
        assert!(
            highest.1 - lowest.1 > 1e-6,
            "every feature drifted by the same amount ({:.6}), so this cannot tell a maximum \
             from a minimum",
            highest.1
        );
        // `>=` and not equality, and the difference is the wire added for
        // §21.1's degraded-model join. The index recorded is now the larger
        // of this per-model reading and the feature's own drift against the
        // estimators' reference, and the second can exceed the first —
        // deliberately, because a wire allowed to *lower* a drift score is a
        // control rewired to fire less than it did before. The property this
        // assertion exists for is untouched: with `highest > lowest` asserted
        // immediately above, a number at or beyond the maximum cannot be a
        // mean, and the mean is the failure it was written against.
        assert!(
            observation.population_stability_index >= highest.1 - 1e-9,
            "the reported index {:.6} is below the largest across features ({:.6} on {}), so \
             something averaged it away",
            observation.population_stability_index,
            highest.1,
            highest.0
        );

        // And the registry now holds that number, so the eligibility check has
        // something to refuse on.
        let card = desk
            .registry()
            .get(&reference)
            .ok_or_else(|| Error::not_found("the card"))?;
        assert!(
            card.drift_score > card.drift_threshold,
            "the measurement did not reach the card: {:.3} against {:.3}",
            card.drift_score,
            card.drift_threshold
        );
        Ok(())
    }

    #[test]
    fn a_feature_that_drifts_past_the_estimators_bound_degrades_every_model_that_reads_it()
    -> Result<()> {
        // §21.1's sentence, as the property rather than as the function:
        // "one that drifts past its bound marks *every* model depending on
        // it as degraded". The failure this guards is a join that returns
        // the first match, or the model the round happened to fit, and
        // leaves a second card reading the same moved feature reported as
        // calm. `degraded_models` was built, tested and called by nothing
        // until this wire, so nothing had ever asserted the "every" part
        // against a registry holding more than one card.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);

        let first = desk
            .maybe_learn(&subject(), &calm, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("the first round"))?;
        // Premise: a desk with no stream reference measures nothing, and
        // reports that by an empty map rather than by a zero index. Without
        // this the assertions below would pass on a desk that never compared
        // anything.
        assert!(
            first.degraded.is_empty(),
            "the first round degraded {} model(s) against no reference at all",
            first.degraded.len()
        );

        // A second fit on the same calm series, so the registry holds two
        // cards reading the same five features before anything moves.
        let second = desk
            .maybe_learn(&subject(), &calm, REGIME, 2, at())?
            .ok_or_else(|| Error::not_found("the second round"))?;
        drop(second);
        assert!(
            desk.registry().len() >= 2,
            "the registry holds {} card(s); the 'every model' property cannot be tested \
             against one",
            desk.registry().len()
        );

        // Now a regime none of them was fitted on.
        let shocked = super::tests_support::shocked(400);
        let third = desk
            .maybe_learn(&subject(), &shocked, REGIME, 3, at())?
            .ok_or_else(|| Error::not_found("the third round"))?;

        // Premise: something actually drifted past the estimators' own
        // floor. An empty drifted set would make every assertion below
        // vacuously true, which is the shape of test this repository has
        // already been burnt by.
        let drifted = third.drifted_features();
        assert!(
            !drifted.is_empty(),
            "no feature moved past the estimators' floor, so this proves nothing about the \
             join; the shocked fixture is not a regime change"
        );

        // The property. Every card naming a drifted feature is in the map,
        // and nothing else is -- except this round's own fit, which joined
        // the registry *after* the comparison ran. Measuring a model against
        // a window that includes the rows it was fitted on is the question
        // the module refuses to ask, so its absence is the behaviour and not
        // a gap in the join.
        let fitted_this_round = third
            .registration
            .as_ref()
            .map(|registration| registration.reference.clone());
        for card in desk.registry().iter() {
            if fitted_this_round.as_deref() == Some(card.reference().as_str()) {
                continue;
            }
            let reads_a_drifted_feature = card
                .features
                .iter()
                .any(|feature| drifted.contains(feature.as_str()));
            assert_eq!(
                reads_a_drifted_feature,
                third.degraded.contains_key(&card.reference()),
                "{} reads {:?}; the drifted set is {:?} and the degraded map {} it",
                card.reference(),
                card.features,
                drifted,
                if third.degraded.contains_key(&card.reference()) {
                    "holds"
                } else {
                    "omits"
                }
            );
        }

        // And the join's own entries name only features the card declares,
        // so a model is never degraded by a feature it does not read.
        for (reference, features) in &third.degraded {
            let card = desk
                .registry()
                .get(reference)
                .ok_or_else(|| Error::not_found(format!("a card for {reference}")))?;
            assert!(
                features.iter().all(|f| card.features.contains(f)),
                "{reference} was degraded by {features:?}, which is not a subset of the {:?} \
                 it reads",
                card.features
            );
        }
        Ok(())
    }

    #[test]
    fn a_model_the_desk_kept_no_sample_for_is_still_degraded_by_a_feature_it_reads() -> Result<()> {
        // The half of §21.1 the per-model comparison structurally cannot
        // reach. A card the desk holds no fitted sample for is invisible to
        // `worst_drift`, so its `drift_score` stays at the 0.0 it was
        // created with for ever and `decision_eligibility`'s drift branch
        // can never refuse it -- the exact shape of `MaxExpectedShortfall`,
        // a control connected to a value nothing writes. The feature-level
        // join is the only thing in this desk that can write it.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        desk.maybe_learn(&subject(), &calm, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("the first round"))?;

        // A card from somewhere else entirely: same feature vocabulary, same
        // instrument, no fitted sample on this desk. The training dataset is
        // what says "same instrument" -- the desk will not join one
        // instrument's drifted features onto a card fitted on another, and a
        // card that names no instrument is reported unmeasurable rather than
        // measured against a guess.
        let stranger = qip_ai::registry::ModelCard::new(
            qip_core::ids::ModelId::from_string("MDL0000000000000000000BBB"),
            "stranger",
            "1.0.0",
            "another-desk",
            at(),
        )
        .with_features(FEATURES.iter().map(|f| (*f).to_string()).collect())
        .with_training_data(vec![dataset_name(&subject())]);
        let stranger_reference = stranger.reference();
        let tracked_before = desk.tracked();
        desk.registry_mut().register(stranger);

        // Premise, in three parts: the desk kept exactly one fitted sample,
        // the registry now holds two cards -- so one of them has no sample
        // and `worst_drift` cannot reach it -- and the stranger's drift
        // score is the zero nobody wrote.
        assert_eq!(
            tracked_before, 1,
            "the desk kept {tracked_before} fitted sample(s); this test needs exactly the one \
             it fitted so that the second card is provably unsampled"
        );
        assert_eq!(
            desk.registry().len(),
            2,
            "the registry holds {} card(s) against one fitted sample",
            desk.registry().len()
        );
        assert_eq!(
            desk.tracked(),
            tracked_before,
            "registering a card gave the desk a fitted sample for it"
        );
        assert_eq!(
            desk.registry()
                .get(&stranger_reference)
                .map(|card| card.drift_score),
            Some(0.0),
            "the stranger arrived with a drift score somebody had already written"
        );
        assert_eq!(
            desk.registry()
                .get(&stranger_reference)
                .and_then(card_subject),
            Some(subject().as_str()),
            "the stranger names no instrument, so the join below would be proving that an \
             unattributed card is measured rather than that an unsampled one is"
        );

        let shocked = super::tests_support::shocked(400);
        let round = desk
            .maybe_learn(&subject(), &shocked, REGIME, 2, at())?
            .ok_or_else(|| Error::not_found("the second round"))?;

        assert!(
            round.degraded.contains_key(&stranger_reference),
            "a regime change left {stranger_reference} undegraded; the degraded map is {:?}",
            round.degraded.keys().collect::<Vec<_>>()
        );
        let score = desk
            .registry()
            .get(&stranger_reference)
            .map(|card| card.drift_score)
            .ok_or_else(|| Error::not_found("the stranger's card"))?;
        assert!(
            score > 0.0,
            "the join named {stranger_reference} as degraded and its card still reads \
             {score:.6}; the measurement did not reach the value the eligibility check reads"
        );
        Ok(())
    }

    #[test]
    fn a_drifted_model_in_production_may_no_longer_inform_a_decision() -> Result<()> {
        // The half that matters to an operator. A development-stage card is
        // refused for its stage before drift is ever read, so only a promoted
        // model can demonstrate that drift is what disqualifies it.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        let reference = desk
            .maybe_learn(&subject(), &calm, REGIME, 1, at())?
            .and_then(|round| round.registration)
            .ok_or_else(|| Error::not_found("a registration"))?
            .reference;

        // Promote it. Refused unless the fit actually cleared the skill bar,
        // which is the point: this cannot be staged for a model with no signal.
        if desk.registry_mut().promote(&reference, at()).is_err() {
            return Err(Error::invalid(
                "the fit did not clear the skill bar, so this test cannot reach the drift \
                 branch; the fixture must produce a learnable series",
            ));
        }
        // The premise: with no drift, this model is decision-eligible. Without
        // asserting it, the refusal below could be about anything.
        desk.registry()
            .require_for_decision(&reference, at())
            .map_err(|error| {
                Error::invalid(format!(
                    "the premise failed: an undrifted, promoted model was already ineligible: {}",
                    error.message()
                ))
            })?;

        let shocked = super::tests_support::shocked(400);
        desk.maybe_learn(&subject(), &shocked, REGIME, 2, at())?;

        let refusal = desk
            .registry()
            .require_for_decision(&reference, at())
            .err()
            .ok_or_else(|| Error::invalid("a drifted model was still allowed to decide"))?;
        assert!(
            refusal.message().contains("drift"),
            "the model was refused for some other reason: {}",
            refusal.message()
        );
        Ok(())
    }

    #[test]
    fn a_disabled_desk_never_learns() {
        // `every_cycles = 0` is a deployment saying it does not want this loop,
        // and it must be distinguishable from a loop that ran and found
        // nothing.
        let desk = LearningDesk::new(
            LearningConfig {
                every_cycles: 0,
                ..LearningConfig::default()
            },
            7,
        );
        assert!(!desk.enabled());
        assert_eq!(desk.tracked(), 0);
        assert_eq!(desk.stats().rounds, 0);
    }

    /// A second regime key, for the property that a precedent earned in one
    /// regime decides nothing in another.
    const OTHER_REGIME: &str = "quiet/calm";

    #[test]
    fn a_round_fits_the_baseline_and_the_challenger_and_scores_each_class_in_the_regime()
    -> Result<()> {
        // §5.4's meta-learning domain — "which model class works in which
        // regime" — had one class to choose from, so the board it needed
        // could never hold a comparison. A round now fits both and tells the
        // board about both, under the regime the platform classified.
        let mut desk = learning_desk();
        // The premise: nothing has been scored before the round.
        assert!(desk.class_board().is_empty());

        let bars = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &bars, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;
        let choice = round
            .class_choice
            .as_ref()
            .ok_or_else(|| Error::not_found("a class choice on a round that registered"))?;

        let baseline = desk
            .class_board()
            .score(BASELINE_CLASS.as_str(), REGIME)
            .ok_or_else(|| Error::not_found("the baseline's score in the regime"))?;
        let challenger = desk
            .class_board()
            .score(CHALLENGER_CLASS.as_str(), REGIME)
            .ok_or_else(|| Error::not_found("the challenger's score in the regime"))?;
        assert_eq!(baseline.observations(), 1);
        assert_eq!(challenger.observations(), 1);
        assert_eq!(
            desk.class_board()
                .score(BASELINE_CLASS.as_str(), OTHER_REGIME),
            None,
            "a round in one regime scored a class in another"
        );
        // What the board was told is what the choice reports, and the
        // challenger was actually fitted rather than refused.
        assert!(
            choice.challenger_skilled.is_some(),
            "the challenger was not fitted on a learnable series"
        );
        let told = if choice.baseline_skilled { 1.0 } else { 0.0 };
        assert!(
            (baseline.observed() - told).abs() < 1e-12,
            "the board was told {} and the choice reports {told}",
            baseline.observed()
        );
        // With no precedent the readable class is registered.
        assert_eq!(choice.registered, BASELINE_CLASS.as_str());
        assert_eq!(choice.reason, ClassReason::NoEstablishedPrecedent);
        assert_eq!(choice.regime, REGIME);
        assert!(
            round.describe().contains("no established precedent"),
            "the round line does not say why the class was chosen: {}",
            round.describe()
        );
        Ok(())
    }

    #[test]
    fn an_established_precedent_for_the_challenger_registers_it_in_that_regime_and_nowhere_else()
    -> Result<()> {
        // The control this board exists to drive: the class that is
        // registered and distilled. Without this, the board would be a
        // record nothing reads — the `MaxExpectedShortfall` shape.
        let mut desk = learning_desk();
        for _ in 0..60 {
            desk.observe_class(&CHALLENGER_CLASS, REGIME, true);
            desk.observe_class(&BASELINE_CLASS, REGIME, false);
        }
        // The premise: the precedent is established by the board's own band
        // and prefers the challenger, in this regime only.
        let precedent = desk
            .class_board()
            .score(CHALLENGER_CLASS.as_str(), REGIME)
            .ok_or_else(|| Error::not_found("the seeded precedent"))?;
        assert!(
            precedent.is_confident(),
            "sixty observations are not established"
        );
        assert_eq!(
            desk.class_board()
                .score(CHALLENGER_CLASS.as_str(), OTHER_REGIME),
            None
        );

        let bars = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &bars, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;
        let choice = round
            .class_choice
            .as_ref()
            .ok_or_else(|| Error::not_found("a class choice on a round that registered"))?;
        assert_eq!(choice.reason, ClassReason::PrecedentPrefersChallenger);
        assert_eq!(choice.registered, CHALLENGER_CLASS.as_str());
        // The card in the registry is the challenger's, not a relabelled
        // baseline: the class is in the name the card was registered under,
        // because nothing else on a card records it.
        let reference = &round
            .registration
            .as_ref()
            .ok_or_else(|| Error::not_found("a registration"))?
            .reference;
        let card = desk
            .registry()
            .get(reference)
            .ok_or_else(|| Error::not_found("the registered card"))?;
        assert_eq!(
            card.name,
            format!("bar-{}-{}", CHALLENGER_CLASS.as_str(), subject().as_str())
        );

        // The same desk, a regime with no precedent: the baseline again.
        let round = desk
            .maybe_learn(&subject(), &bars, OTHER_REGIME, 2, at())?
            .ok_or_else(|| Error::not_found("a second round"))?;
        let choice = round
            .class_choice
            .as_ref()
            .ok_or_else(|| Error::not_found("a class choice on the second round"))?;
        assert_eq!(choice.reason, ClassReason::NoEstablishedPrecedent);
        assert_eq!(choice.registered, BASELINE_CLASS.as_str());
        Ok(())
    }

    #[test]
    fn an_established_precedent_that_does_not_beat_the_baseline_keeps_the_baseline() -> Result<()> {
        // A tie is not a preference. The readable class is registered unless
        // the other one has actually done better in this regime; a board
        // that flipped on equal evidence would be choosing on noise.
        let mut desk = learning_desk();
        for _ in 0..60 {
            desk.observe_class(&CHALLENGER_CLASS, REGIME, true);
            desk.observe_class(&BASELINE_CLASS, REGIME, true);
        }
        let challenger = desk
            .class_board()
            .score(CHALLENGER_CLASS.as_str(), REGIME)
            .ok_or_else(|| Error::not_found("the seeded challenger precedent"))?;
        let baseline = desk
            .class_board()
            .score(BASELINE_CLASS.as_str(), REGIME)
            .ok_or_else(|| Error::not_found("the seeded baseline precedent"))?;
        // The premise: established, and equal.
        assert!(challenger.is_confident());
        assert!((challenger.score() - baseline.score()).abs() < 1e-12);

        let bars = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &bars, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;
        let choice = round
            .class_choice
            .as_ref()
            .ok_or_else(|| Error::not_found("a class choice on a round that registered"))?;
        assert_eq!(choice.reason, ClassReason::PrecedentBelowBaseline);
        assert_eq!(choice.registered, BASELINE_CLASS.as_str());
        Ok(())
    }

    #[test]
    fn a_round_with_no_regime_key_is_refused_rather_than_scored_under_one_context() {
        // An empty key would fold every regime into one cell and the board
        // would learn which class works in no regime at all.
        let mut desk = learning_desk();
        let bars = super::tests_support::learnable(400);
        let refused = desk.learn_window(&subject(), &bars, "  ", at());
        assert!(refused.is_err(), "an empty regime key was accepted");
        assert!(
            desk.class_board().is_empty(),
            "a refused round still scored"
        );
    }

    #[test]
    fn a_round_that_registers_a_teacher_also_carries_a_student_distilled_on_its_holdout_tail()
    -> Result<()> {
        // `qip_training::distill::distil` was fully implemented, tested in
        // its own crate, and reachable from no running process: nothing ever
        // turned a registered teacher into the `DistilledModel` the
        // execution path is allowed to run. This proves the wiring reaches
        // it, and that the probe set it is scored on is genuinely the same
        // holdout tail the teacher's own diagnostics were scored against
        // rather than a second, disconnected split.
        let mut desk = learning_desk();
        let bars = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &bars, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;

        let registration = round
            .registration
            .as_ref()
            .ok_or_else(|| Error::not_found("a registration to distil from"))?;

        // The premise: a teacher was actually registered this round, so a
        // `None` distillation below would mean nothing was attempted rather
        // than something failed.
        assert!(
            !registration.reference.is_empty(),
            "the registration carries no reference to distil against"
        );

        let distillation = round.distillation.as_ref().ok_or_else(|| {
            Error::not_found(format!(
                "a distillation; refused because: {:?}",
                round.distillation_refusal
            ))
        })?;
        assert!(
            round.distillation_refusal.is_none(),
            "a distillation is present and a refusal reason is too -- at most one of the two \
             should ever be set"
        );
        assert_eq!(
            distillation.teacher_reference(),
            registration.reference,
            "the student was distilled from a teacher other than the one this round registered"
        );
        assert_eq!(
            distillation.form(),
            StudentForm::Linear { ridge: 1e-3 },
            "the student's form does not match what the desk asked to distil"
        );

        // The probe set actually used must be the same size as the holdout
        // tail `TrainingDataset::split_at_fraction` would carve from this
        // subject's dataset with the desk's configured fraction -- not the
        // full dataset, and not the fit set.
        let rows = bars.len() - LOOKBACK - 1;
        let expected_holdout =
            ((rows as f64) * LearningConfig::default().holdout_fraction).round() as usize;
        let expected_holdout = expected_holdout.clamp(1, rows - 1);
        assert_eq!(
            distillation.fidelity().probe_samples,
            expected_holdout,
            "the student was probed on {} row(s), not the {} the fit's own holdout tail holds",
            distillation.fidelity().probe_samples,
            expected_holdout
        );
        Ok(())
    }

    #[test]
    fn a_holdout_tail_too_small_to_fit_a_student_reports_why_rather_than_a_silent_none()
    -> Result<()> {
        // `distillation: None` alone cannot distinguish "no teacher this
        // round" from "distillation was attempted and refused". Forcing a
        // holdout so small the linear student cannot be determined proves
        // the refusal reason reaches the round rather than being swallowed.
        let mut desk = LearningDesk::new(
            LearningConfig {
                every_cycles: 1,
                minimum_bars: 64,
                // Five percent of ~53 fitted rows rounds to a probe of a
                // handful of observations -- fewer than the five features
                // plus an intercept a linear student needs to be determined,
                // while the teacher's own diagnostics (which impose no
                // minimum) still fit without complaint.
                holdout_fraction: 0.05,
                ..LearningConfig::default()
            },
            7,
        );
        let bars = super::tests_support::learnable(64);
        let round = desk
            .maybe_learn(&subject(), &bars, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;

        // The premise: a teacher was still registered this round, so the
        // refusal below is about the student and not about the fit never
        // having happened at all.
        assert!(
            round.registration.is_some(),
            "the fixture must still produce a teacher for this to test the distillation path \
             and not just an unfit round"
        );

        assert!(
            round.distillation.is_none(),
            "a student was distilled from a probe set too small to determine its coefficients"
        );
        let reason = round.distillation_refusal.ok_or_else(|| {
            Error::not_found(
                "a reason distillation produced nothing, distinguishing a refusal from a round \
                 that never attempted one",
            )
        })?;
        assert!(
            reason.contains("coefficient"),
            "the refusal reason does not name why the fit could not be determined: {reason}"
        );
        Ok(())
    }

    #[test]
    fn a_drifted_feature_degrades_only_the_models_fitted_on_the_instrument_it_drifted_on()
    -> Result<()> {
        // The feature-level join, which is the second of the two routes a
        // drift score can reach a card by and the one a per-model reference
        // cannot fix. `return_1` moving on one instrument says nothing about
        // `return_1` on another, so joining this round's drifted feature set
        // onto every card in the registry marks models degraded on evidence
        // from a series they have never seen -- and `decision_eligibility`
        // refuses on the score that join writes.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        let first = desk
            .maybe_learn(&subject(), &calm, REGIME, 1, at())?
            .and_then(|round| round.registration)
            .ok_or_else(|| Error::not_found("a registration on the first subject"))?
            .reference;
        let second = desk
            .maybe_learn(&other_subject(), &calm, REGIME, 2, at())?
            .and_then(|round| round.registration)
            .ok_or_else(|| Error::not_found("a registration on the second subject"))?
            .reference;

        // The premise, in three parts: two distinct cards are standing, they
        // are attributed to different instruments, and they read the same
        // features -- so the join below has something to reach them both by
        // and nothing but the subject can tell them apart.
        assert_ne!(
            first, second,
            "both rounds registered the same card, so there is only one model here"
        );
        assert_eq!(
            desk.registry().get(&first).and_then(card_subject),
            Some(subject().as_str())
        );
        assert_eq!(
            desk.registry().get(&second).and_then(card_subject),
            Some(other_subject().as_str())
        );
        assert_eq!(
            desk.registry()
                .get(&first)
                .map(|card| card.features.clone()),
            desk.registry()
                .get(&second)
                .map(|card| card.features.clone()),
            "the two cards read different features, so the join could separate them without \
             ever consulting the instrument"
        );

        // The second instrument, and only the second, changes regime.
        let shocked = super::tests_support::shocked(400);
        let round = desk
            .maybe_learn(&other_subject(), &shocked, REGIME, 3, at())?
            .ok_or_else(|| Error::not_found("a round on the second subject"))?;

        // The premise for the exclusion: the join did fire, on the card fitted
        // where the features actually moved.
        assert!(
            round.degraded.contains_key(&second),
            "the join degraded nothing on the instrument that moved, so the exclusion below \
             proves nothing; degraded is {:?}",
            round.degraded.keys().collect::<Vec<_>>()
        );
        assert!(
            !round.degraded.contains_key(&first),
            "a model fitted on {} was degraded by features that drifted on {}",
            subject().as_str(),
            other_subject().as_str()
        );
        assert_eq!(
            desk.registry().get(&first).map(|card| card.drift_score),
            Some(0.0),
            "the join wrote a drift score onto a card fitted on an instrument this round \
             never looked at"
        );
        Ok(())
    }

    #[test]
    fn a_card_naming_no_instrument_is_reported_unmeasurable_rather_than_measured_against_a_guess()
    -> Result<()> {
        // The other half of the subject fix, and the half that could have
        // become a silent gap. A card the desk cannot attribute to an
        // instrument is measured by neither route, so its drift score stays
        // the zero nobody wrote and `decision_eligibility` reads it as
        // undrifted forever. That is the `MaxExpectedShortfall` shape, so the
        // desk names the card on the round rather than letting it disappear.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        desk.maybe_learn(&subject(), &calm, REGIME, 1, at())?
            .ok_or_else(|| Error::not_found("the first round"))?;

        // Same features, same everything -- except it says nothing about what
        // it was fitted on.
        let orphan = qip_ai::registry::ModelCard::new(
            qip_core::ids::ModelId::from_string("MDL0000000000000000000CCC"),
            "orphan",
            "1.0.0",
            "another-desk",
            at(),
        )
        .with_features(FEATURES.iter().map(|f| (*f).to_string()).collect());
        let orphan_reference = orphan.reference();
        desk.registry_mut().register(orphan);

        // The premise: the card really does name no instrument, and really is
        // in the registry the round walks.
        assert_eq!(
            desk.registry()
                .get(&orphan_reference)
                .and_then(card_subject),
            None,
            "the fixture card names an instrument, so it is not the case under test"
        );
        assert_eq!(
            desk.registry().len(),
            2,
            "the registry holds {} card(s); the orphan is not in the set the round walks",
            desk.registry().len()
        );

        let shocked = super::tests_support::shocked(400);
        let round = desk
            .maybe_learn(&subject(), &shocked, REGIME, 2, at())?
            .ok_or_else(|| Error::not_found("the second round"))?;

        // Premise for the absence: the round's drift pass did fire, on the
        // card it could attribute.
        assert!(
            !round.degraded.is_empty(),
            "no model was degraded at all, so the orphan's absence proves nothing"
        );
        assert!(
            !round.degraded.contains_key(&orphan_reference),
            "a card naming no instrument was degraded by this instrument's drifted features"
        );
        assert_eq!(
            desk.registry()
                .get(&orphan_reference)
                .map(|card| card.drift_score),
            Some(0.0),
            "an unattributable card was given a drift score anyway"
        );
        assert!(
            round.unattributed.contains(&orphan_reference),
            "the round measured nothing for {orphan_reference} and said nothing about it \
             either; the round names {:?}",
            round.unattributed
        );
        assert!(
            round
                .describe()
                .contains("measurable against no instrument"),
            "the round's own line hides the unmeasurable card: {}",
            round.describe()
        );
        Ok(())
    }

    #[test]
    fn a_model_is_never_measured_for_drift_against_bars_of_a_subject_it_was_not_fitted_on()
    -> Result<()> {
        // The deep brain rotates subjects: `EvolutionEngine::maybe_learn`
        // picks whichever subject currently carries the greatest notional
        // depth, and re-takes that choice every round. This desk held one
        // reference sample per model and compared every one of them against
        // whatever window the round brought, so a model fitted on one
        // instrument was scored for drift against another instrument's bars.
        // `ModelCard::decision_eligibility` refuses on that score, so a model
        // could be ruled out of decisions by a number that was about a
        // different instrument entirely -- which is worse than no number,
        // because it reads as a measurement.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        let reference = desk
            .maybe_learn(&subject(), &calm, REGIME, 1, at())?
            .and_then(|round| round.registration)
            .ok_or_else(|| Error::not_found("a registration on the first subject"))?
            .reference;

        // The premise, in two parts: the model is standing and the desk holds
        // its sample, and its drift score is the zero nobody wrote.
        assert_eq!(
            desk.tracked(),
            1,
            "the desk kept no fitted sample, so nothing below could be measured either way"
        );
        assert_eq!(
            desk.registry().get(&reference).map(|card| card.drift_score),
            Some(0.0),
            "the card began with a drift score somebody had already written"
        );

        // A different instrument, in a regime the first one never visited.
        let shocked = super::tests_support::shocked(400);
        let second = desk
            .maybe_learn(&other_subject(), &shocked, REGIME, 2, at())?
            .ok_or_else(|| Error::not_found("a round on the second subject"))?;

        // The premise that makes the absence below mean something: the round
        // really ran its drift pass and really fitted, so a missing
        // observation is a refusal to measure rather than a round that did
        // nothing at all.
        assert!(
            second.registration.is_some(),
            "the second round fitted nothing: {:?}",
            second.ineligible
        );
        assert!(
            !second
                .drift
                .iter()
                .any(|observation| { observation.reference == reference }),
            "a model fitted on {} was scored for drift against {}'s bars: {:?}",
            subject().as_str(),
            other_subject().as_str(),
            second
                .drift
                .iter()
                .map(|observation| (
                    observation.reference.clone(),
                    observation.population_stability_index
                ))
                .collect::<Vec<_>>()
        );
        assert!(
            !second.degraded.contains_key(&reference),
            "the feature-level join degraded a model using another instrument's drifted \
             features: {:?}",
            second.degraded.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            desk.registry().get(&reference).map(|card| card.drift_score),
            Some(0.0),
            "the card's drift score moved on a round that never saw its instrument"
        );

        // The complement, without which the assertions above would also pass
        // if drift measurement had simply been switched off: the same shock
        // on the model's *own* subject is measured, and past its threshold.
        let third = desk
            .maybe_learn(&subject(), &shocked, REGIME, 3, at())?
            .ok_or_else(|| Error::not_found("a round back on the first subject"))?;
        let observation = third
            .drift
            .iter()
            .find(|observation| observation.reference == reference)
            .ok_or_else(|| {
                Error::not_found("drift measured against the model's own subject's bars")
            })?;
        assert!(
            observation.above_threshold,
            "a regime change on the model's own instrument produced a stability index of \
             {:.3}, which did not pass its threshold",
            observation.population_stability_index
        );
        // And the stream-level reference is per subject too. Round two shocked
        // a different instrument; had that replaced this one's reference
        // distribution -- as a single global reference did -- the third round
        // would compare shocked bars against shocked estimators and the
        // feature-level join would find nothing to degrade.
        assert!(
            third.degraded.contains_key(&reference),
            "the round on the model's own instrument degraded nothing: a round on another \
             instrument moved this one's stream reference; degraded is {:?}",
            third.degraded.keys().collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn a_feature_row_never_reads_the_bar_its_target_spans() {
        // Point-in-time. Row `i` is computed from bars up to `LOOKBACK + i`,
        // and its target is the return from that bar to the next -- so the
        // label always sits on the far side of everything used to predict it.
        // A dataset without this property produces a backtest nobody should
        // trust and there is no way to recover it afterwards.
        let bars = super::tests_support::rising(40);
        let columns = feature_columns(&bars);
        let targets = next_bar_returns(&bars);
        // The premise: rows were produced at all.
        let returns = columns.get("return_1").expect("the return column exists");
        assert!(!returns.is_empty(), "no feature row was produced");
        assert_eq!(
            returns.len(),
            targets.len(),
            "the feature rows and targets describe different numbers of instants"
        );

        // Row 0 reads bars[LOOKBACK] and its target spans LOOKBACK -> +1.
        let expected = ratio(
            bars[LOOKBACK].close.to_f64(),
            bars[LOOKBACK - 1].close.to_f64(),
        );
        assert!(
            (returns[0] - expected).abs() < 1e-12,
            "the first row's return is not the one ending at its own bar"
        );
        let target = ratio(
            bars[LOOKBACK + 1].close.to_f64(),
            bars[LOOKBACK].close.to_f64(),
        );
        assert!(
            (targets[0] - target).abs() < 1e-12,
            "the first target is not the return into the next bar"
        );
    }
}

#[cfg(test)]
mod tests_support {
    use qip_core::rng::{Rng, Xoshiro256};
    use qip_core::{Decimal, ObjectId, Timestamp};
    use qip_financial::quality::DataQuality;
    use qip_market::bar::{Bar, Interval};

    /// A series whose next return is genuinely predictable from its last.
    ///
    /// Returns follow `r(t+1) = 0.6 * r(t) + noise`, so a linear fit on
    /// `return_1` explains a real share of out-of-sample variance and the skill
    /// bar can be cleared honestly. A constant ramp cannot: its returns have no
    /// variance, so there is nothing for a holdout R-squared to be a share of,
    /// and a model that "passed" on it would have passed on nothing.
    pub(super) fn learnable(count: usize) -> Vec<Bar> {
        let mut rng = Xoshiro256::seeded(0x1EA2);
        let mut bars = rising(count);
        let mut level = 100.0_f64;
        let mut previous = 0.0_f64;
        for bar in bars.iter_mut() {
            let shock = rng.uniform(-0.004, 0.004);
            let ret = 0.6 * previous + shock;
            previous = ret;
            level *= 1.0 + ret;
            bar.close = Decimal::from_f64(level).unwrap_or(Decimal::ONE);
            bar.open = Decimal::from_f64(level * (1.0 - ret / 2.0)).unwrap_or(Decimal::ONE);
            bar.high = Decimal::from_f64(level * 1.001).unwrap_or(Decimal::ONE);
            bar.low = Decimal::from_f64(level * 0.999).unwrap_or(Decimal::ONE);
        }
        bars
    }

    /// A series whose next return is independent of everything before it.
    ///
    /// The other half of the skill bar: a fit here has nothing to find, so its
    /// out-of-sample R-squared sits at or below zero and the verdict must say
    /// so. Written as fresh draws rather than a shuffle, because a shuffle of a
    /// predictable series preserves its marginal distribution and can leave
    /// enough structure to score above a low bar by luck.
    pub(super) fn unlearnable(count: usize) -> Vec<Bar> {
        let mut rng = Xoshiro256::seeded(0x0B5C_0DE5);
        let mut bars = rising(count);
        let mut level = 100.0_f64;
        for bar in bars.iter_mut() {
            let ret = rng.uniform(-0.01, 0.01);
            level *= 1.0 + ret;
            bar.close = Decimal::from_f64(level).unwrap_or(Decimal::ONE);
            bar.open = Decimal::from_f64(level * (1.0 - ret / 2.0)).unwrap_or(Decimal::ONE);
            bar.high = Decimal::from_f64(level * 1.002).unwrap_or(Decimal::ONE);
            bar.low = Decimal::from_f64(level * 0.998).unwrap_or(Decimal::ONE);
        }
        bars
    }

    /// A series in a regime the calm one never visits: violent alternating
    /// jumps instead of a steady drift, so every return-based feature lands in
    /// a distribution the reference sample has no mass in.
    pub(super) fn shocked(count: usize) -> Vec<Bar> {
        let mut bars = rising(count);
        let mut level = 100.0_f64;
        for (i, bar) in bars.iter_mut().enumerate() {
            level *= if i % 2 == 0 { 1.06 } else { 0.945 };
            bar.close = Decimal::from_f64(level).unwrap_or(Decimal::ONE);
            bar.open = Decimal::from_f64(level * 0.99).unwrap_or(Decimal::ONE);
            bar.high = Decimal::from_f64(level * 1.05).unwrap_or(Decimal::ONE);
            bar.low = Decimal::from_f64(level * 0.95).unwrap_or(Decimal::ONE);
            bar.volume = Decimal::from_f64(if i % 2 == 0 { 40_000.0 } else { 120.0 })
                .unwrap_or(Decimal::ONE);
        }
        bars
    }

    /// A steadily rising series with real volume, enough to build features on.
    pub(super) fn rising(count: usize) -> Vec<Bar> {
        (0..count)
            .map(|i| {
                let close = 100.0 + i as f64 * 0.5;
                Bar {
                    object_id: ObjectId::from_string("OBJ0000000000000000000AAA"),
                    venue: "XNYS".to_string(),
                    interval: Interval::Minute,
                    open_time: Timestamp::from_secs(1_760_000_000 + i as i64 * 60),
                    open: Decimal::from_f64(close * 0.999).unwrap_or(Decimal::ONE),
                    high: Decimal::from_f64(close * 1.002).unwrap_or(Decimal::ONE),
                    low: Decimal::from_f64(close * 0.998).unwrap_or(Decimal::ONE),
                    close: Decimal::from_f64(close).unwrap_or(Decimal::ONE),
                    volume: Decimal::from_f64(1_000.0 + i as f64).unwrap_or(Decimal::ONE),
                    trade_count: 100,
                    vwap: Decimal::from_f64(close),
                    quality: DataQuality::default(),
                }
            })
            .collect()
    }
}
