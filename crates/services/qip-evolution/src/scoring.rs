//! Scoring, in context, with the sample size never dropped.
//!
//! Five things are scored here — strategies, models, sources, regimes and
//! execution — and all five are scored the same way, by one engine with five
//! named constructors rather than five copies of the same arithmetic. The
//! shared shape is not a convenience: it is the claim that these are the same
//! kind of statement. Each is "how often did this go well, *in what
//! circumstance*, and how much have we seen".
//!
//! Two rules hold for every score.
//!
//! **A score is never a single number.** It is a value per context, because
//! the useful facts are all conditional: a strategy that works in a calm
//! regime and loses in a volatile one, a source that is timely on equities and
//! late on credit, a venue that fills small orders well and large ones badly.
//! Averaging those into one figure destroys exactly the information a
//! decision needs. [`Scoreboard::pooled`] exists for the cases where an
//! aggregate is genuinely wanted, and its documentation says what it hides.
//!
//! **A score shrinks toward its prior when the sample is small.** The weight
//! is [`qip_contracts::Conviction::shrunk`]'s, recovered from it rather than
//! restated, so the two cannot drift apart. Three good days do not produce a
//! confident score, and the band on the score says so in words rather than
//! leaving a reader to notice the observation count.

use qip_contracts::Conviction;
use std::collections::BTreeMap;

/// What is being scored.
///
/// Carried on the score so a number cannot be read out of the domain that
/// gives it meaning — an 0.8 for a source and an 0.8 for a venue are not
/// comparable and should not look it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScoreDomain {
    /// A strategy: how often its signals were right, by regime.
    Strategy,
    /// A model: how often its predictions held, by regime.
    Model,
    /// A data source: how often it was timely and correct, by dataset or
    /// asset class.
    Source,
    /// A regime: how well the platform as a whole did while it lasted.
    Regime,
    /// Execution: how often a fill landed inside what was modelled, by venue
    /// and order size.
    Execution,
}

impl ScoreDomain {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Strategy => "strategy",
            Self::Model => "model",
            Self::Source => "source",
            Self::Regime => "regime",
            Self::Execution => "execution",
        }
    }

    pub const fn all() -> [Self; 5] {
        [
            Self::Strategy,
            Self::Model,
            Self::Source,
            Self::Regime,
            Self::Execution,
        ]
    }
}

/// How much a score should be believed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScoreBand {
    /// Too little evidence to have moved off the prior. Fewer than ten
    /// observations.
    Unproven,
    /// Enough to be worth looking at, not enough to act on alone. Up to
    /// roughly forty-five observations.
    Indicative,
    /// Enough that the observed value is most of what the score says.
    Established,
}

impl ScoreBand {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unproven => "unproven",
            Self::Indicative => "indicative",
            Self::Established => "established",
        }
    }

    /// Whether a score in this band may be acted on without corroboration.
    pub const fn is_confident(&self) -> bool {
        matches!(self, Self::Established)
    }
}

/// The weight [`Conviction::shrunk`] gives to `observations` worth of
/// evidence.
///
/// Recovered from `Conviction` rather than restated: feeding it a certainty of
/// one gives `prior + weight * (1 - prior)` with a prior of one half, so the
/// weight falls straight out. If the platform ever changes how quickly a
/// belief earns its keep, it changes in one place and every score here follows.
pub fn evidence_weight(observations: u32) -> f64 {
    (Conviction::new(1.0, observations).shrunk() - 0.5) * 2.0
}

/// One score, for one subject, in one context.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextualScore {
    domain: ScoreDomain,
    subject: String,
    context: String,
    observed: f64,
    observations: u32,
    prior: f64,
}

impl ContextualScore {
    pub const fn domain(&self) -> ScoreDomain {
        self.domain
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The circumstance this score is about — a regime, a venue, an asset
    /// class. Never empty: a score with no context is a score nobody can use.
    pub fn context(&self) -> &str {
        &self.context
    }

    /// The raw rate, before shrinkage. Reported so the shrinkage is visible.
    pub const fn observed(&self) -> f64 {
        self.observed
    }

    pub const fn observations(&self) -> u32 {
        self.observations
    }

    pub const fn prior(&self) -> f64 {
        self.prior
    }

    /// How much of the observed value the score keeps.
    pub fn evidence_weight(&self) -> f64 {
        evidence_weight(self.observations)
    }

    /// The score: the observed rate pulled toward the prior by how little
    /// evidence stands behind it.
    pub fn score(&self) -> f64 {
        let weight = self.evidence_weight();
        self.prior + weight * (self.observed - self.prior)
    }

    /// The same statement as a [`Conviction`], for handing to anything that
    /// already speaks that language.
    ///
    /// Exact only where the prior is one half, which is the default and the
    /// case `Conviction` itself assumes.
    pub fn conviction(&self) -> Conviction {
        Conviction::new(self.observed, self.observations)
    }

    pub fn band(&self) -> ScoreBand {
        let weight = self.evidence_weight();
        if weight < 0.25 {
            ScoreBand::Unproven
        } else if weight < 0.6 {
            ScoreBand::Indicative
        } else {
            ScoreBand::Established
        }
    }

    /// Whether this score may be acted on without corroboration.
    pub fn is_confident(&self) -> bool {
        self.band().is_confident()
    }

    pub fn summarise(&self) -> String {
        format!(
            "{} {} in {}: {:.2} from {} observation(s), shrunk from {:.2} toward {:.2} ({})",
            self.domain.as_str(),
            self.subject,
            self.context,
            self.score(),
            self.observations,
            self.observed,
            self.prior,
            self.band().as_str()
        )
    }
}

/// One thing that happened, and how well it went.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    subject: String,
    context: String,
    value: f64,
}

impl Outcome {
    /// `value` is "how well this went" in `[0, 1]`: one for a signal that was
    /// right, a fill inside the modelled cost, a datum that arrived on time.
    ///
    /// Clamps rather than refusing, following [`Conviction::new`]: a caller
    /// that computes 1.02 has a bug worth finding, and dropping the
    /// observation loses the evidence as well.
    pub fn new(subject: impl Into<String>, context: impl Into<String>, value: f64) -> Self {
        Self {
            subject: subject.into(),
            context: context.into(),
            value: if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            },
        }
    }

    /// A binary outcome — it went well or it did not.
    pub fn binary(subject: impl Into<String>, context: impl Into<String>, went_well: bool) -> Self {
        Self::new(subject, context, if went_well { 1.0 } else { 0.0 })
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub const fn value(&self) -> f64 {
        self.value
    }
}

/// Running total for one subject in one context.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tally {
    total: f64,
    count: u32,
}

impl Tally {
    fn observed(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.total / f64::from(self.count)
    }
}

/// Scores for one domain, kept per subject and per context.
#[derive(Clone, Debug, PartialEq)]
pub struct Scoreboard {
    domain: ScoreDomain,
    prior: f64,
    cells: BTreeMap<(String, String), Tally>,
}

impl Scoreboard {
    /// A board with an explicit prior.
    ///
    /// The prior is what a subject with no evidence is worth. One half is the
    /// honest default — "we do not know" — and a domain with a real base rate
    /// should say so rather than inherit it.
    pub fn new(domain: ScoreDomain, prior: f64) -> Self {
        Self {
            domain,
            prior: prior.clamp(0.0, 1.0),
            cells: BTreeMap::new(),
        }
    }

    /// How often a strategy's signals were right, by regime.
    pub fn strategies() -> Self {
        Self::new(ScoreDomain::Strategy, 0.5)
    }

    /// How often a model's predictions held, by regime.
    pub fn models() -> Self {
        Self::new(ScoreDomain::Model, 0.5)
    }

    /// How often a source was timely and correct, by dataset or asset class.
    pub fn sources() -> Self {
        Self::new(ScoreDomain::Source, 0.5)
    }

    /// How the platform did while a regime lasted.
    pub fn regimes() -> Self {
        Self::new(ScoreDomain::Regime, 0.5)
    }

    /// How often a fill landed inside what was modelled, by venue and size.
    pub fn execution() -> Self {
        Self::new(ScoreDomain::Execution, 0.5)
    }

    pub const fn domain(&self) -> ScoreDomain {
        self.domain
    }

    pub const fn prior(&self) -> f64 {
        self.prior
    }

    pub fn observe(&mut self, outcome: Outcome) {
        let cell = self
            .cells
            .entry((outcome.subject().to_string(), outcome.context().to_string()))
            .or_default();
        cell.total += outcome.value();
        cell.count += 1;
    }

    /// Record several at once.
    pub fn observe_all(&mut self, outcomes: impl IntoIterator<Item = Outcome>) {
        for outcome in outcomes {
            self.observe(outcome);
        }
    }

    /// The score for one subject in one context, or `None` where nothing has
    /// been observed.
    ///
    /// `None` rather than a prior-valued score: "we have never seen this" and
    /// "we have seen it and learned nothing" are different facts, and a caller
    /// that wants to treat them alike can.
    pub fn score(&self, subject: &str, context: &str) -> Option<ContextualScore> {
        let tally = self
            .cells
            .get(&(subject.to_string(), context.to_string()))?;
        Some(self.render(subject, context, *tally))
    }

    /// Every context this subject has been seen in, in a stable order.
    pub fn scores_of(&self, subject: &str) -> Vec<ContextualScore> {
        self.cells
            .iter()
            .filter(|((held, _), _)| held == subject)
            .map(|((held, context), tally)| self.render(held, context, *tally))
            .collect()
    }

    /// One number for a subject across every context it has been seen in.
    ///
    /// What it hides is the reason the rest of this module exists: a strategy
    /// scoring 0.8 in a calm regime and 0.2 in a volatile one pools to 0.5,
    /// which describes neither and reads as "average". Use it for a headline,
    /// never for a decision — [`Self::spread`] is the number that says whether
    /// the pool is lying.
    pub fn pooled(&self, subject: &str) -> Option<ContextualScore> {
        let mut pooled = Tally::default();
        let mut seen = false;
        for ((held, _), tally) in &self.cells {
            if held != subject {
                continue;
            }
            seen = true;
            pooled.total += tally.total;
            pooled.count += tally.count;
        }
        seen.then(|| self.render(subject, "pooled", pooled))
    }

    /// The gap between a subject's best and worst context, after shrinkage.
    ///
    /// A large spread means the pooled score is an average of two different
    /// facts and should not be quoted.
    pub fn spread(&self, subject: &str) -> Option<f64> {
        let scores = self.scores_of(subject);
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        for score in &scores {
            let value = score.score();
            low = low.min(value);
            high = high.max(value);
        }
        (!scores.is_empty()).then_some(high - low)
    }

    /// The context a subject does best in, where it has been seen at all.
    pub fn best_context(&self, subject: &str) -> Option<ContextualScore> {
        self.scores_of(subject).into_iter().reduce(|best, next| {
            if next.score() > best.score() {
                next
            } else {
                best
            }
        })
    }

    /// Every subject on the board, in canonical order.
    pub fn subjects(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .cells
            .keys()
            .map(|(subject, _)| subject.as_str())
            .collect();
        names.dedup();
        names
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    fn render(&self, subject: &str, context: &str, tally: Tally) -> ContextualScore {
        ContextualScore {
            domain: self.domain,
            subject: subject.to_string(),
            context: context.to_string(),
            observed: tally.observed(),
            observations: tally.count,
            prior: self.prior,
        }
    }
}
