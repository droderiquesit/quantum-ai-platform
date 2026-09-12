//! The evolution brain, turning: search, score against history, register.
//!
//! Until this module, every driver of the strategy foundry was a test. The
//! grammar could write candidates, the compiler could admit them, the trial
//! ledger could count them — and no running process ever asked. The audit's
//! phrase was "the evolution brain never turns", and this is the crank.
//!
//! One round, on a cadence measured in research cycles:
//!
//! 1. **Sense.** The engine polls its data adapter, hands every record to
//!    [`Platform::observe`] — the research node ran blind before this, which
//!    its own cycle reports said plainly — and tees closed bars into a bounded
//!    per-instrument history.
//! 2. **Search.** The foundry proposes candidates over the bar-derivable
//!    catalogue. The same catalogue drives generation, compilation and
//!    evaluation, so a candidate that references anything the harness cannot
//!    compute fails compilation before it can waste an evaluation.
//! 3. **Score.** Each admitted candidate runs through the backtester on the
//!    subject's own observed history — real bars this process saw, not a
//!    fixture — producing holdout returns, purged folds and a leakage audit
//!    built from the harness's own trace.
//! 4. **Register.** Survivors enter the strategy factory on the bottom rung
//!    with the search's true trial count attached. Promotion stays the
//!    factory's and the gates' decision; nothing here can move a strategy up
//!    a rung, which is the property that makes an automated search safe to
//!    leave running.
//!
//! # What a round refuses
//!
//! Too little history is not evidence: a subject below the minimum bar count
//! is skipped, and a candidate whose holdout would be shorter than the
//! configured floor is discarded rather than registered on a sliver. The
//! trial count still rises for every scored candidate, discarded or not —
//! that is the whole point of the ledger.

use crate::learning::{LearningConfig, LearningDesk, LearningRound, LearningStats};
use crate::succession::{ChallengeSummary, SuccessionDesk, SuccessionStats};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_evolution::challenger::ChallengerPolicy;
use qip_evolution::cost_model::NetReturns;
use qip_evolution::generate::Candidate;
use qip_evolution::grammar::Grammar;
use qip_evolution::palette::FeaturePalette;
use qip_financial::universe::Universe;
use qip_kernel::central::foundry::{HoldoutInputs, StrategyFoundry};
use qip_kernel::platform::Platform;
use qip_lifecycle::evidence::{CrossValidationRun, FeatureTiming, LeakageAudit};
use qip_market::bar::Bar;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_simulation_engine::backtest::{BacktestConfig, Backtester};
use qip_simulation_engine::clock::{ExecutionAssumptions, SimulationClock};
use qip_simulation_engine::harness::{CompiledHarness, WARM_UP_BARS, bar_catalogue};
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::compile::StrategyCompiler;
use std::collections::BTreeMap;

/// How the evolution loop is tuned. Every knob is a policy with a reason,
/// not a magic number discovered in a constructor.
#[derive(Clone, Debug)]
pub struct EvolutionConfig {
    /// Run a round every this many research cycles. Zero disables the loop
    /// entirely, which is a deployment's honest way of saying "search
    /// elsewhere" rather than an undocumented environment variable.
    pub every_cycles: u64,
    /// Candidates proposed per round. Small on purpose: the trial ledger
    /// deflates every holdout by the search size, so an enormous round buys
    /// mostly deflation.
    pub candidates: usize,
    /// Bars a subject must have before it is worth searching on. Below this
    /// the holdout is a sliver and every verdict is noise.
    pub minimum_bars: usize,
    /// Fraction of the return series held out as the final unseen tail.
    pub holdout_fraction: f64,
    /// The largest weight the evaluation lets a signal take.
    pub max_weight: f64,
    /// Bars kept per instrument. Bounded so a long-running research node's
    /// memory is a function of configuration, not uptime.
    pub history_cap: usize,
    /// Challengers minted from the champion each round.
    ///
    /// Small for the same reason `candidates` is, and for one more: every
    /// challenger is folded into the same trial ledger, so a large challenge
    /// round deflates the holdout of everything the generative search finds
    /// alongside it.
    pub challengers: usize,
    /// How the learning desk is tuned.
    ///
    /// Carried here rather than hard-coded inside the desk so a deployment can
    /// turn it off and a test can reach it. A knob nothing can set is a
    /// constant with a longer name.
    pub learning: LearningConfig,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            every_cycles: 4,
            candidates: 8,
            minimum_bars: minimum_bars_for_the_holdout_gate(0.25),
            holdout_fraction: 0.25,
            max_weight: 0.5,
            history_cap: 2048,
            challengers: 4,
            learning: LearningConfig::default(),
        }
    }
}

impl EvolutionConfig {
    /// Read the two operator-facing knobs from the environment.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let mut config = Self::default();
        if let Some(raw) = lookup("QIP_DEEPBRAIN_EVOLUTION_EVERY") {
            config.every_cycles = raw.trim().parse().map_err(|_| {
                Error::invalid(format!(
                    "QIP_DEEPBRAIN_EVOLUTION_EVERY is {raw:?}, not a cycle count; 0 disables \
                     the evolution loop"
                ))
            })?;
        }
        if let Some(raw) = lookup("QIP_DEEPBRAIN_EVOLUTION_CANDIDATES") {
            let parsed: usize = raw.trim().parse().map_err(|_| {
                Error::invalid(format!(
                    "QIP_DEEPBRAIN_EVOLUTION_CANDIDATES is {raw:?}, not a candidate count"
                ))
            })?;
            if parsed == 0 {
                return Err(Error::invalid(
                    "a round of zero candidates is not a smaller search; set \
                     QIP_DEEPBRAIN_EVOLUTION_EVERY=0 to disable the loop instead",
                ));
            }
            config.candidates = parsed;
        }
        Ok(config)
    }
}

/// The most refusal reasons one round's summary carries.
///
/// A bound rather than none: the reasons reach an operator's cycle line, and
/// a round that refused a hundred candidates would otherwise write a hundred
/// paragraphs to it.
const MAX_REFUSALS_REPORTED: usize = 4;

/// The least history a search may run on, derived from the gate that will
/// judge what it finds.
///
/// The holdout gate refuses a Sharpe computed on fewer than
/// `HoldoutPolicy::minimum_observations` held-out observations — a year of
/// daily bars, below which the standard error is too wide to say anything.
/// Only `holdout_fraction` of a run's bars are held out, so the run needs the
/// whole of that divided by the fraction before it can possibly clear the
/// check.
///
/// # Why this is derived and not chosen
///
/// It was `WARM_UP_BARS * 6` — about 188 bars, which holds out 47 against a
/// minimum of 250. Every round before roughly a thousand bars therefore
/// registered candidates the gate *structurally could not admit*, and that
/// was not merely wasted work: every candidate is charged to the family's
/// cumulative trial count, and the count deflates the Sharpe every later
/// candidate is judged against. A search that cannot pass still raises the
/// bar for the searches that could, so the early futile rounds were making
/// the later real ones harder, permanently and silently. That is the
/// multiple-comparisons problem the trial book exists to police, arriving
/// through the one door nobody was watching.
///
/// Derived from `HoldoutPolicy` rather than written down again, so a change
/// to the gate's minimum moves this with it. Two constants that must agree
/// and are maintained apart are two constants that will eventually disagree.
fn minimum_bars_for_the_holdout_gate(holdout_fraction: f64) -> usize {
    let needed = qip_lifecycle::gates::HoldoutPolicy::default().minimum_observations;
    let from_gate = (needed as f64 / holdout_fraction).ceil() as usize;
    // The harness still needs its warm-up before the first bar a strategy can
    // act on, so the floor is whichever is larger.
    from_gate.max(WARM_UP_BARS * 6)
}

/// What one round did, for the operator's cycle line.
#[derive(Clone, Debug, Default)]
pub struct RoundSummary {
    pub subject: String,
    pub proposed: usize,
    pub admitted: usize,
    pub registered: usize,
    pub discarded: usize,
    /// Candidates the holdout gate admitted, so they left `Candidate` for the
    /// first rung of the ladder.
    pub promoted: usize,
    /// Why the gate refused, one entry per refusal up to
    /// [`MAX_REFUSALS_REPORTED`].
    pub gate_refusals: Vec<String>,
    /// Candidates the holdout gate refused. Not a failure of the round: the
    /// gate doing its work is the point, and a search whose every candidate
    /// is refused is a search that found nothing worth funding — which is a
    /// result, and one nothing recorded before this counted it.
    pub gate_refused: usize,
    /// The search's cumulative trial count after this round — the number
    /// every holdout is deflated by.
    pub trials: usize,
    /// What the champion/challenger contest concluded, where one ran.
    pub challenge: Option<ChallengeSummary>,
}

impl RoundSummary {
    pub fn describe(&self) -> String {
        format!(
            "evolution: {} proposed {} on {}, {} admitted, {} registered, {} discarded, \
             {} promoted, {} refused at the gate, {} trial(s) on the ledger",
            self.proposed,
            if self.proposed == 1 {
                "candidate"
            } else {
                "candidates"
            },
            self.subject,
            self.admitted,
            self.registered,
            self.discarded,
            self.promoted,
            self.gate_refused,
            self.trials
        )
    }
}

/// Running totals across the node's lifetime, for the shutdown report.
#[derive(Clone, Copy, Debug, Default)]
pub struct EvolutionStats {
    pub rounds: u64,
    pub registered: u64,
    pub discarded: u64,
}

/// The crank. Owns the data adapter, the per-subject histories and foundries.
pub struct EvolutionEngine {
    config: EvolutionConfig,
    adapter: Box<dyn DataAdapter>,
    seed: u64,
    /// The instruments a backtest may trade.
    ///
    /// The same universe the platform was assembled with, and not a fresh
    /// empty one. A backtest against an empty universe rejects every order as
    /// an unknown instrument, fills nothing, and returns a perfectly flat
    /// equity curve -- which the gate then reads as a strategy with no
    /// volatility rather than as a strategy that never traded.
    universe: Universe,
    history: BTreeMap<String, Vec<Bar>>,
    foundries: BTreeMap<String, StrategyFoundry>,
    desk: SuccessionDesk,
    learning: LearningDesk,
    stats: EvolutionStats,
    /// Source discovery (§7.4-§7.6.2), on its own cadence. `None` is the
    /// default and every deployment today: this engine states no view on
    /// where a candidate list comes from, and [`Self::with_discovery`] is
    /// the one door that attaches one.
    discovery: Option<crate::discovery::DiscoveryDesk>,
}

impl std::fmt::Debug for EvolutionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvolutionEngine")
            .field("config", &self.config)
            .field("subjects", &self.history.len())
            .field("champions", &self.desk.champions())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl EvolutionEngine {
    /// Build the crank.
    ///
    /// Refuses a configuration whose challenge round is empty rather than
    /// treating it as "challenges off": a desk that mints nothing still holds
    /// champions and still reports rounds, which reads as a comparison that
    /// happened and found nothing. `every_cycles = 0` is how a deployment says
    /// it does not want this loop, and it says so visibly.
    pub fn new(
        config: EvolutionConfig,
        adapter: Box<dyn DataAdapter>,
        seed: u64,
        universe: Universe,
    ) -> Result<Self> {
        let desk = SuccessionDesk::new(config.challengers, ChallengerPolicy::default())?;
        let config_learning = config.learning.clone();
        Ok(Self {
            config,
            adapter,
            seed,
            universe,
            history: BTreeMap::new(),
            foundries: BTreeMap::new(),
            desk,
            learning: LearningDesk::new(config_learning, seed),
            stats: EvolutionStats::default(),
            discovery: None,
        })
    }

    /// Attach a discovery desk (§7.4-§7.6.2). Replaces one attached before.
    pub fn with_discovery(mut self, desk: crate::discovery::DiscoveryDesk) -> Self {
        self.discovery = Some(desk);
        self
    }

    /// Run a discovery pass if this cycle is on its cadence and a desk is
    /// attached; `Ok(None)` otherwise, for either reason.
    pub fn maybe_discover(
        &mut self,
        platform: &mut Platform,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<qip_kernel::SourceAssessment>> {
        match &mut self.discovery {
            Some(desk) => desk.maybe_run(platform, cycle, now),
            None => Ok(None),
        }
    }

    /// Build the crank over the synthetic exchange, with the exchange's own
    /// instruments as the reference universe.
    ///
    /// The composition root's constructor for the default deployment path.
    /// Before it, `main.rs` passed `Universe::new()` with a comment saying the
    /// node had no reference-data source yet — and against an empty universe
    /// the backtester rejects every order as an unknown instrument, the
    /// no-fill refusal discards every candidate, and the loop is off. Taking
    /// the environment by value here, before it is boxed behind
    /// `dyn DataAdapter`, is what lets the reference data and the bars come
    /// from the one instrument list; after boxing, the list is unreachable.
    pub fn over_synthetic(
        config: EvolutionConfig,
        environment: qip_market_ingestion::synthetic::SyntheticEnvironment,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let universe = crate::reference::synthetic_universe(&environment, at)?;
        Self::new(config, Box::new(environment), seed, universe)
    }

    /// What the succession desk has done across the node's lifetime.
    pub const fn succession_stats(&self) -> SuccessionStats {
        self.desk.stats()
    }

    /// What the learning desk has done across the node's lifetime.
    pub const fn learning_stats(&self) -> LearningStats {
        self.learning.stats()
    }

    /// Fit a model on the deepest subject and measure the standing ones for
    /// drift, on the learning desk's own cadence.
    ///
    /// Separate from [`Self::maybe_turn`] and on its own cadence, because
    /// fitting a function and searching for a strategy answer different
    /// questions at different rates. They share a subject: the one the node can
    /// most readily trade is also the one whose bars carry the most signal to
    /// learn from.
    pub fn maybe_learn(&mut self, cycle: u64, now: Timestamp) -> Result<Option<LearningRound>> {
        let Some((subject, bars)) = self
            .history
            .iter()
            .filter_map(|(subject, bars)| depth(bars).map(|depth| (subject, bars, depth)))
            .max_by(|left, right| left.2.total_cmp(&right.2))
            .map(|(subject, bars, _)| (ObjectId::from_string(subject), bars.clone()))
        else {
            return Ok(None);
        };
        self.learning.maybe_learn(&subject, &bars, cycle, now)
    }

    pub const fn stats(&self) -> EvolutionStats {
        self.stats
    }

    pub const fn enabled(&self) -> bool {
        self.config.every_cycles > 0
    }

    /// Whether the adapter owns time — a tape — so the loop takes each
    /// cycle's instant from [`Self::advance`] rather than the wall clock.
    pub fn owns_time(&self) -> bool {
        self.adapter.owns_time()
    }

    /// Move a time-owning adapter to its next instant and return it, or
    /// `None` when it is spent. See `DataAdapter::advance`.
    pub fn advance(&mut self) -> Option<Timestamp> {
        self.adapter.advance()
    }

    /// Poll the adapter, feed the platform, tee the bars. Returns how many
    /// records the platform absorbed, for the cycle line.
    pub fn sense(&mut self, platform: &mut Platform, until: Timestamp) -> Result<usize> {
        let records = self.adapter.poll(until)?;
        for record in &records {
            if let SensedRecord::Bar(bar) = record {
                let bars = self
                    .history
                    .entry(bar.object_id.as_str().to_string())
                    .or_default();
                bars.push((**bar).clone());
                if bars.len() > self.config.history_cap {
                    let excess = bars.len() - self.config.history_cap;
                    bars.drain(..excess);
                }
            }
        }
        Ok(platform.observe(records))
    }

    /// Run one round if the cadence says so and any subject has enough
    /// history. `None` means "not this cycle", which is normal, or "no
    /// subject is ready", which the caller's cycle line makes visible by the
    /// absence of an evolution line.
    pub fn maybe_turn(
        &mut self,
        platform: &mut Platform,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<RoundSummary>> {
        if self.config.every_cycles == 0 || cycle % self.config.every_cycles != 0 {
            return Ok(None);
        }
        let Some((subject, bars)) = self
            .history
            .iter()
            .filter(|(_, bars)| bars.len() >= self.config.minimum_bars)
            // Ranked by liquidity, not by bar count. Every subject on this
            // feed accumulates bars at the same rate, so ranking by count is
            // really ranking by key order -- and the synthetic exchange's
            // illiquid government bond sorts last, so every round this node had
            // ever run searched the instrument whose orders the impact model
            // most often refuses. Thin bars give rejected orders, rejected
            // orders give a flat equity curve, and a flat curve is a round that
            // spent its trials on a question with no answer.
            //
            // The statistic is the one `tradeable_capital` sizes against, so
            // "where can we trade" and "how much can we trade" are one rule
            // rather than two that happen to agree.
            .filter_map(|(subject, bars)| depth(bars).map(|depth| (subject, bars, depth)))
            .max_by(|left, right| left.2.total_cmp(&right.2))
            .map(|(subject, bars, _)| (subject.clone(), bars.clone()))
        else {
            return Ok(None);
        };

        let summary = self.turn(platform, &subject, bars, now)?;
        self.stats.rounds += 1;
        self.stats.registered += summary.registered as u64;
        self.stats.discarded += summary.discarded as u64;
        Ok(Some(summary))
    }

    fn turn(
        &mut self,
        platform: &mut Platform,
        subject: &str,
        bars: Vec<Bar>,
        now: Timestamp,
    ) -> Result<RoundSummary> {
        let object = ObjectId::from_string(subject);
        let foundry = match self.foundries.entry(subject.to_string()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let catalogue = bar_catalogue(&object)?;
                let grammar = Grammar::over(FeaturePalette::from_catalogue(
                    &bar_catalogue(&object)?,
                    &object,
                )?);
                // The seed is folded with the subject so two subjects search
                // different corners, and the foundry folds it into the
                // identity prefix so two searches cannot mint colliding ids.
                let seed = self.seed ^ fold(subject);
                entry.insert(StrategyFoundry::new(
                    catalogue,
                    grammar,
                    "central-research",
                    VenueId::new("XSIM"),
                    format!("evo-{subject}"),
                    seed,
                )?)
            }
        };

        let round = foundry.search(self.config.candidates)?;
        let mut summary = RoundSummary {
            subject: subject.to_string(),
            proposed: round.requested,
            admitted: round.accepted,
            trials: round.trials,
            ..RoundSummary::default()
        };

        // The foundry holds the arena its candidates were compiled into and
        // does not lend it out. Rather than reaching into the kernel for an
        // accessor, each candidate's *spec* — which is public precisely so a
        // byte-identity check across runs is possible — is recompiled into a
        // local arena for evaluation. The compiler is deterministic over the
        // same catalogue, and the harness's own plan-versus-arena check would
        // refuse the pairing if that ever stopped being true.
        let pending: Vec<Candidate> = foundry.pending().to_vec();
        let object = ObjectId::from_string(subject);
        let mut admitted: Vec<Candidate> = Vec::new();
        for candidate in pending {
            let id = candidate.id().clone();
            let scored = self.evaluate(&object, candidate.spec(), &bars);
            let foundry = self
                .foundries
                .get_mut(subject)
                .ok_or_else(|| Error::not_found("the foundry that just searched"))?;
            match scored {
                Ok(scored) => {
                    match foundry.register(
                        platform.central_mut().factory_mut(),
                        &id,
                        scored.holdout,
                        now,
                    ) {
                        Ok(()) => {
                            summary.registered += 1;
                            // Put the evidence the search just produced in
                            // front of the gate that exists to judge it.
                            //
                            // Nothing did this before, and the consequence was
                            // the whole ladder standing still: `promote` had
                            // no caller outside tests, so every candidate this
                            // loop registered stayed a `Candidate` for ever,
                            // no baseline was ever written, and §20.3's
                            // retirement machinery — which skips any strategy
                            // without a pilot baseline — observed nothing in
                            // any deployment. A pipeline that searches,
                            // scores and registers, and then never asks the
                            // question it gathered the evidence for, reads as
                            // a working lifecycle and is a dead end.
                            //
                            // One rung only, and deliberately. The gate for
                            // each stage reads that stage's own evidence, and
                            // the foundry attaches holdout evidence alone; the
                            // rungs above want paper and shadow records the
                            // strategy has not had the chance to make yet.
                            // Walking further would be asking gates to rule on
                            // evidence nobody gathered, and their refusals
                            // would say exactly that, at length, every round.
                            //
                            // No approval is passed because none is required:
                            // `GateStage::requires_human_approval` is true for
                            // `Pilot` and `Scaled` only, which are the rungs
                            // where capital is at stake, and neither is
                            // reachable from here.
                            match platform.central_mut().factory_mut().promote(
                                &id,
                                None,
                                "the holdout gate read the search's own evidence",
                                now,
                            ) {
                                Ok(_) => summary.promoted += 1,
                                // A refused gate is an outcome, not an error,
                                // and it must not leave this loop: `?` here
                                // would abandon the rest of the round because
                                // one candidate was judged and found wanting,
                                // which is the gate working. Only `Guard` is
                                // the gate speaking — every other class is a
                                // fault in the caller (an unregistered
                                // strategy, a ledger that cannot take the
                                // record) and is returned rather than counted
                                // as a verdict nobody reached.
                                Err(qip_core::Error::Guard(why)) => {
                                    summary.gate_refused += 1;
                                    // The reason travels with the count. A
                                    // round that says two candidates were
                                    // refused and not why is a round an
                                    // operator cannot act on, and the
                                    // difference between "the gate is doing
                                    // its job" and "the evidence is malformed
                                    // and every candidate is refused for a
                                    // reason nobody looked at" is exactly
                                    // what these strings carry. Bounded,
                                    // because a round's refusals reach a
                                    // summary line and an unbounded field is
                                    // an unbounded record.
                                    if summary.gate_refusals.len() < MAX_REFUSALS_REPORTED {
                                        summary.gate_refusals.push(why);
                                    }
                                }
                                Err(error) => return Err(error),
                            }
                            admitted.push(candidate);
                        }
                        Err(_) => {
                            foundry.discard(&id);
                            summary.discarded += 1;
                        }
                    }
                }
                Err(_) => {
                    foundry.discard(&id);
                    summary.discarded += 1;
                }
            }
        }
        summary.challenge = Some(self.contest(platform, subject, &object, &bars, &admitted, now)?);
        // Read after the contest: a challenge round mints challengers, and
        // every one of them is a trial. Reading the count before would report
        // a smaller search than the one the holdouts were deflated by.
        if let Some(foundry) = self.foundries.get(subject) {
            summary.trials = foundry.trials();
        }
        Ok(summary)
    }

    /// Install a first champion, or mount a challenge against the one there is.
    ///
    /// Runs after registration because a champion has to be a strategy the
    /// factory's gate admitted. A candidate the gate refused is not a champion
    /// that happens to be unproven; it is one the platform has already declined.
    fn contest(
        &mut self,
        platform: &mut Platform,
        subject: &str,
        object: &ObjectId,
        bars: &[Bar],
        admitted: &[Candidate],
        now: Timestamp,
    ) -> Result<ChallengeSummary> {
        let mut summary = ChallengeSummary {
            subject: subject.to_string(),
            ..ChallengeSummary::default()
        };

        // A demoted champion must stop being the parent every challenger is
        // mutated from. A search descending from a strategy the platform has
        // already stopped trusting is the quiet way an automated loop keeps
        // investing in a dead line.
        summary.refusals.extend(
            self.desk
                .retire_demoted(platform.central().factory().ledger()),
        );

        if self.desk.champion(subject).is_none() {
            // The bootstrap. Deliberately the *first* candidate the gate
            // admitted this round and not the highest-scoring one: picking the
            // best of a fresh search is precisely the multiple-comparisons
            // error the trial ledger exists to deflate, and a bootstrap has
            // nothing to deflate against. The challenge loop is what improves
            // on it, under a count.
            let Some(first) = admitted.first() else {
                return Ok(summary);
            };
            match self
                .desk
                .install_first(platform.central().factory().ledger(), object, first, now)
            {
                Ok(succession) => summary.crowned = Some(succession.champion.to_string()),
                Err(error) => summary.refusals.push(error.message().to_string()),
            }
            return Ok(summary);
        }

        let Some(champion) = self.desk.champion(subject).cloned() else {
            return Ok(summary);
        };

        // Champion and challengers are compiled into one arena, over the same
        // catalogue, so a challenger is type-checked against exactly the
        // vocabulary its parent was.
        let mut compiler = StrategyCompiler::new(bar_catalogue(object)?);
        let grammar = Grammar::over(FeaturePalette::from_catalogue(
            &bar_catalogue(object)?,
            object,
        )?);
        // A different corner of the stream from the generative search on the
        // same subject, so the two are not drawing the same edits.
        let seed = self.seed ^ fold(subject) ^ 0x9e37_79b9_7f4a_7c15;

        let run = {
            let Self {
                desk, foundries, ..
            } = self;
            let foundry = foundries
                .get_mut(subject)
                .ok_or_else(|| Error::not_found("the foundry that just searched"))?;
            desk.mint(subject, foundry, &mut compiler, grammar, seed)
        };
        let Some(run) = run else {
            return Ok(summary);
        };
        summary.minted = run.accepted().len() + run.rejected().len();
        summary.refused = run.rejected().len();

        // The champion is re-scored on this round's window rather than
        // compared against whatever it scored when it was crowned. A number
        // from an older, shorter window is not evidence about this one, and
        // the challenger test refuses two series of different lengths for
        // exactly that reason.
        let champion_net = match self.evaluate(object, champion.spec(), bars) {
            Ok(scored) => scored.net,
            Err(error) => {
                summary
                    .refusals
                    .push(format!("the champion could not be re-scored: {error}"));
                return Ok(summary);
            }
        };

        // Scored first, judged second: scoring needs the engine immutably and
        // judging needs the desk mutably.
        let mut scored: Vec<(usize, NetReturns)> = Vec::new();
        for (index, challenger) in run.accepted().iter().enumerate() {
            match self.evaluate(object, challenger.candidate().spec(), bars) {
                Ok(result) => scored.push((index, result.net)),
                Err(error) => summary.refusals.push(error.message().to_string()),
            }
        }

        let ledger = platform.central().factory().ledger();
        // Read once, off the live platform, and handed to every judgement in
        // this round rather than recomputed per challenger: two challengers
        // scored in the same round are scored in the same regime, and a
        // recomputed read could disagree with itself mid-round if a sensed
        // observation landed between them.
        let regime = platform.regime_context(subject);
        let Self {
            desk, foundries, ..
        } = self;
        let foundry = foundries
            .get(subject)
            .ok_or_else(|| Error::not_found("the foundry that just searched"))?;
        for (index, net) in scored {
            let Some(challenger) = run.accepted().get(index) else {
                continue;
            };
            match desk.judge(
                ledger,
                object,
                challenger,
                champion_net.clone(),
                net,
                PERIODS_PER_YEAR,
                foundry,
                &regime,
                now,
            ) {
                Ok(Some(succession)) => {
                    summary.compared += 1;
                    summary.winners += 1;
                    summary.crowned = Some(succession.champion.to_string());
                    // One succession per round. A second would crown a
                    // challenger of a champion that no longer reigns, which is
                    // a comparison against something that is no longer there.
                    break;
                }
                Ok(None) => summary.compared += 1,
                // A refusal is not a loss. "The comparison could not be made"
                // and "the challenger lost" are different facts, and counting
                // the first as the second would make the champion look like it
                // keeps surviving.
                Err(error) => summary.refusals.push(error.message().to_string()),
            }
        }
        Ok(summary)
    }

    /// Score one candidate on the subject's own history.
    ///
    /// Returns the gate's evidence and the same holdout tail as a cost-charged
    /// series, because the challenger test refuses a window that was charged
    /// nothing -- a strategy that traded for a year and paid nothing has a
    /// backtest, not a result.
    fn evaluate(
        &self,
        subject: &ObjectId,
        spec: &qip_strategy::ir::StrategySpec,
        bars: &[Bar],
    ) -> Result<Scored> {
        let mut compiler = qip_strategy::compile::StrategyCompiler::new(bar_catalogue(subject)?);
        let compiled = compiler.compile(spec)?;
        let mut harness =
            CompiledHarness::new(compiled, compiler.into_program(), self.config.max_weight)?;
        // The lag is the data's own spacing, not a day.
        // `ExecutionAssumptions::next_bar` hard-codes `Duration::from_days(1)`
        // and is documented as the default *for a daily strategy*; these bars
        // are minutes apart. Asked for a day of lag on minute bars, every
        // decision in the final 1,440 bars became unexecutable and every
        // earlier one was superseded before it came due -- two thousand
        // rebalances, zero fills, and a flat equity curve that the holdout
        // gate scored as a real result. `allow_same_bar_fill` stays false, so
        // this is still "decide on this bar, trade at the next" and never on
        // the bar that produced the signal.
        let mut clock = SimulationClock::new(
            bars.to_vec(),
            ExecutionAssumptions::intraday(bar_interval(bars)),
        )?;
        // Capital sized to the data, not to an institutional default.
        // `BacktestConfig::default` starts with ten million, which against a
        // one-minute bar of this synthetic exchange is a full-weight order
        // worth three thousand times the volume traded in the window. The
        // impact model refuses it -- correctly, since the square-root law is
        // calibrated on modest participation and extrapolating it "would
        // return a number rather than an answer" -- so every order was
        // rejected and the equity curve was flat. A backtest whose orders
        // cannot execute against the observed volume is measuring the size,
        // not the strategy.
        let config = BacktestConfig {
            initial_capital: tradeable_capital(bars, self.config.max_weight)?,
            ..BacktestConfig::default()
        };
        let result = Backtester::new(config)?.run(&mut harness, &mut clock, &self.universe)?;

        // A run that never traded is not a strategy with no volatility; it is
        // no evidence at all. Refusing here is what stops a flat line reaching
        // the gate as a Sharpe ratio, and it names the two causes worth
        // checking rather than reporting a bare zero.
        if result.fills.is_empty() {
            return Err(Error::invalid(format!(
                "{} rebalance(s) produced no fill over {} period(s): {} order(s) were refused \
                 and {} were superseded before they came due. A holdout in which nothing \
                 traded is not evidence, whatever Sharpe ratio it computes",
                result.rebalance_count,
                result.returns.len(),
                result.rejected.len(),
                result.superseded,
            )));
        }

        let charges = period_charges(&result);
        let returns = result.returns.clone();
        let holdout_len = ((returns.len() as f64) * self.config.holdout_fraction) as usize;
        if holdout_len < WARM_UP_BARS {
            return Err(Error::invalid(format!(
                "a holdout of {holdout_len} return(s) is a sliver, not evidence; the subject \
                 needs more history before a verdict means anything"
            )));
        }
        let split = returns.len() - holdout_len;
        let (pre, tail) = returns.split_at(split);

        // The folds come from `PurgedSplit`, the same splitter the holdout
        // gate rebuilds them with, and the counts reported below are the
        // splitter's own.
        //
        // They were hand-rolled here until 2026-09-08, and the arithmetic
        // disagreed: this counted a fold's leading purge *and* its trailing
        // label horizon as purged, where the splitter counts the horizon once,
        // so a three-fold split over the same data reported four purged where
        // the splitter found two. The gate compares the two and refused every
        // candidate on `purging_and_embargo_applied` — correctly, because a
        // run whose folds cannot be rebuilt from its own description is a run
        // nobody can check. The defect was never in the gate; it was two
        // implementations of one split, which is what the gate exists to
        // catch. There is now one, and this arm cannot drift from it again
        // because it no longer has arithmetic of its own to drift with.
        const FOLDS: usize = 3;
        const LABEL_HORIZON: usize = 1;
        const EMBARGO: usize = 1;
        let splits = PurgedSplit::new(FOLDS, LABEL_HORIZON, EMBARGO)
            .and_then(|split| split.split(pre.len()))
            .map_err(|error| {
                Error::invalid(format!(
                    "the pre-holdout region of {} observation(s) cannot be split into {FOLDS} \
                     purged folds: {}",
                    pre.len(),
                    error.message()
                ))
            })?;
        let mut in_sample = Vec::with_capacity(splits.len());
        let mut out_of_sample = Vec::with_capacity(splits.len());
        let mut purged = 0usize;
        let mut embargoed = 0usize;
        for split in &splits {
            in_sample.push(split.train.iter().map(|&i| pre[i]).collect::<Vec<f64>>());
            out_of_sample.push(split.test.iter().map(|&i| pre[i]).collect::<Vec<f64>>());
            purged += split.purged;
            embargoed += split.embargoed;
        }

        let trace = harness.trace();
        let timings = trace
            .timings
            .iter()
            .map(|(feature, (used_at, known_at))| FeatureTiming {
                feature: feature.clone(),
                known_at: *known_at,
                used_at: *used_at,
            })
            .collect();

        // The charge series is built alongside the equity curve the returns
        // come from, so the two are aligned by construction and the tail slice
        // is the same window on both.
        let charged = &charges[split..];
        let gross: Vec<f64> = tail
            .iter()
            .zip(charged)
            .map(|(net, charge)| net + charge)
            .collect();
        let net = NetReturns::of(&gross, charged)?;

        let holdout = HoldoutInputs {
            returns: tail.to_vec(),
            in_sample_folds: in_sample,
            out_of_sample_folds: out_of_sample,
            periods_per_year: PERIODS_PER_YEAR,
            cross_validation: CrossValidationRun {
                folds: FOLDS,
                label_horizon: LABEL_HORIZON,
                embargo: EMBARGO,
                observations: pre.len(),
                purged,
                embargoed,
            },
            leakage: LeakageAudit {
                timings,
                restated_without_snapshots: Vec::new(),
            },
        };
        Ok(Scored { holdout, net })
    }
}

/// One candidate scored: the gate's evidence, and the holdout tail as a
/// cost-charged series.
struct Scored {
    holdout: HoldoutInputs,
    net: NetReturns,
}

/// Recover, per period, the cost the equity curve already absorbed.
///
/// The backtester's returns are net: the fills' commission, spread and impact
/// are inside the equity curve before the return is taken. `NetReturns` wants
/// the deduction visible next to the result it changed, so the charge is
/// reconstructed here from the fills rather than asserted -- and
/// `NetReturns::is_costless` can then notice a window that paid nothing, which
/// is a cost model that is switched off rather than a strategy that is free.
///
/// A fill at instant `t` belongs to the period ending at the first curve point
/// at or after it, matching `equity_returns`, which takes each return over the
/// interval between consecutive curve points. The charge is expressed in
/// return units by dividing by the equity the period started from -- the same
/// denominator the return itself uses.
///
/// This is the crossing point between money and statistics: fills carry
/// `Decimal` costs, and everything downstream of here is `f64` because a
/// Sharpe ratio is not money.
fn period_charges(result: &qip_simulation_engine::backtest::BacktestResult) -> Vec<f64> {
    let curve = &result.equity_curve;
    let periods = curve.len().saturating_sub(1);
    let mut charges = vec![0.0; periods];
    if periods == 0 {
        return charges;
    }
    for fill in &result.fills {
        // The period whose closing instant is the first at or after the fill.
        let index = curve.partition_point(|(at, _)| *at < fill.at);
        if index == 0 || index > periods {
            continue;
        }
        let opening = curve[index - 1].1.to_f64();
        if opening.abs() < 1e-12 {
            continue;
        }
        charges[index - 1] += fill.cost.total() / opening;
    }
    charges
}

/// The capital a full-weight position can be taken with against this data.
///
/// A position of `max_weight` must sit inside the impact model's participation
/// limit on the bars the strategy actually trades on, or the order is refused
/// and the run measures nothing.
///
/// Sized off the tenth percentile of traded notional rather than the median,
/// because the binding case is the thin bar and not the typical one: at the
/// median, nine tenths of a run clears and the tenth is refused, which is
/// enough to leave a candidate with four fills over a hundred and eighty
/// decisions. `PARTICIPATION` is then half the impact model's own 20% ceiling,
/// so an ordinary bar has room to spare.
///
/// A quantile rather than the mean: one opening print many times the typical
/// size would drag a mean upward and size every order in the run off a bar
/// that happened once.
///
/// Refuses rather than defaults when the data has no volume at all. A run
/// against an instrument nothing traded cannot produce evidence, and quietly
/// choosing a capital figure would produce a backtest that looks like one.
fn tradeable_capital(bars: &[Bar], max_weight: f64) -> Result<Decimal> {
    /// Half the impact model's own ceiling, leaving room on a thin bar.
    const PARTICIPATION: f64 = 0.10;

    let mut traded: Vec<f64> = bars
        .iter()
        .map(|bar| bar.volume.to_f64() * bar.close.to_f64())
        .filter(|notional| *notional > 0.0)
        .collect();
    if traded.is_empty() {
        return Err(Error::invalid(format!(
            "no bar of the {} observed traded at all; an instrument nothing traded cannot be \
             backtested, and choosing a capital figure anyway would produce a run that looks \
             like evidence",
            bars.len()
        )));
    }
    traded.sort_by(f64::total_cmp);
    // The tenth percentile of the bars that traded, not the median: the
    // binding case is the thin bar, not the typical one. At the median, nine
    // tenths of a run clears and the tenth is refused -- measured, that left a
    // candidate with four fills across a hundred and eighty decisions.
    //
    // Quiet bars are not a defect at this frequency. An order landing on one
    // is refused for that bar and the run continues; what must not happen is
    // every order being refused, which is what an institutional default does
    // against a one-minute bar.
    let thin = traded[traded.len() / 10];
    // `max_weight` is the ceiling the harness scales conviction against, so
    // this is the largest position a candidate can ask for.
    let weight = if max_weight > 0.0 { max_weight } else { 1.0 };
    Decimal::from_f64(thin * PARTICIPATION / weight).ok_or_else(|| {
        Error::numeric(format!(
            "a thin-bar notional of {thin} does not yield a representable capital figure"
        ))
    })
}

/// How much this instrument trades: mean notional across every bar.
///
/// The statistic a subject is *chosen* by, which is a different question from
/// the one it is *sized* by above. Depth asks "where can a search produce
/// evidence at all"; the thin-bar percentile asks "how large an order clears
/// there". Averaging across every bar rather than only the traded ones is what
/// makes the difference visible: an instrument quiet in nine bars of ten has a
/// tenth the depth of one that trades continuously, however similar their
/// traded bars look.
///
/// `None` when nothing traded in any bar.
fn depth(bars: &[Bar]) -> Option<f64> {
    if bars.is_empty() {
        return None;
    }
    let total: f64 = bars
        .iter()
        .map(|bar| bar.volume.to_f64() * bar.close.to_f64())
        .sum();
    (total > 0.0).then(|| total / bars.len() as f64)
}

/// The spacing of the data, taken as the gap between the first two bars.
///
/// Used as the execution lag so a decision trades at the *next* bar whatever
/// the bar is. Falls back to a day when there is nothing to measure, matching
/// [`ExecutionAssumptions::next_bar`] -- a single bar cannot be backtested
/// anyway, and the minimum-history bar refuses long before this is reached.
fn bar_interval(bars: &[Bar]) -> qip_core::Duration {
    match bars {
        [first, second, ..] => qip_core::Duration::from_secs(
            second
                .close_time()
                .as_secs()
                .saturating_sub(first.close_time().as_secs())
                .max(1),
        ),
        _ => qip_core::Duration::from_days(1),
    }
}

/// Daily bars. Stated once so the holdout evidence and the challenger
/// comparison annualise on the same basis -- two annualisations of the same
/// series that disagree produce two Sharpe ratios for one strategy.
const PERIODS_PER_YEAR: f64 = 252.0;

/// A tiny deterministic fold of a subject name into the seed. Not a hash
/// with any properties beyond "different names, almost always different
/// corners" — collisions cost nothing but two subjects sharing a search
/// stream.
fn fold(subject: &str) -> u64 {
    subject.bytes().fold(0xcbf2_9ce4_8422_2325u64, |acc, byte| {
        (acc ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_contracts::FeatureKey;
    use qip_contracts::SignalKind;
    use qip_core::{Context, Duration};
    use qip_evolution::cost_model::NetReturns;
    use qip_financial::universe::Universe;
    use qip_kernel::config::PlatformConfig;
    use qip_lifecycle::ledger::LifecycleLedger;
    use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;
    use qip_strategy::ir::{Expr, Rule, StrategySpec};

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Result<Platform> {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
    }

    fn engine(every: u64) -> EvolutionEngine {
        engine_with(EvolutionConfig {
            every_cycles: every,
            ..EvolutionConfig::default()
        })
    }

    fn engine_with(config: EvolutionConfig) -> EvolutionEngine {
        let step = Duration::from_mins(1);
        let synthetic = EnvironmentConfig {
            seed: 7,
            step,
            ..EnvironmentConfig::default()
        };
        // The production constructor, deliberately: every round test in this
        // module then runs against the reference universe a deployment gets,
        // not a fixture that could quietly diverge from it.
        EvolutionEngine::over_synthetic(
            config,
            SyntheticEnvironment::demo(start(), synthetic),
            7,
            start(),
        )
        .expect("the demo environment yields a reference universe")
    }

    /// Sense enough minutes that a subject crosses the minimum-history bar,
    /// then run `rounds` rounds, feeding more history between each.
    fn run_rounds(rounds: u64) -> Result<(Platform, EvolutionEngine, Vec<RoundSummary>)> {
        let mut platform = platform()?;
        let mut engine = engine(1);
        let mut now = start();
        for _ in 0..((engine.config.minimum_bars as i64 + 30) * 2) {
            now = now.saturating_add(Duration::from_mins(1));
            engine.sense(&mut platform, now)?;
        }
        let mut summaries = Vec::new();
        for round in 1..=rounds {
            if let Some(summary) = engine.maybe_turn(&mut platform, round, now)? {
                summaries.push(summary);
            }
            for _ in 0..120 {
                now = now.saturating_add(Duration::from_mins(1));
                engine.sense(&mut platform, now)?;
            }
        }
        Ok((platform, engine, summaries))
    }

    #[test]
    fn a_round_registers_only_when_the_reference_universe_is_populated() -> Result<()> {
        // The reference-data gap, driven end to end. The composition root
        // assembled this engine with `Universe::new()` because the node had no
        // reference-data source: the backtester then rejected every order as
        // an unknown instrument, the no-fill refusal discarded every
        // candidate, and the loop was visibly off. The same feed is run
        // through both assemblies below, so the universe is the only
        // difference between the round that registers and the round that
        // cannot.
        let step = Duration::from_mins(1);
        let synthetic = EnvironmentConfig {
            seed: 7,
            step,
            ..EnvironmentConfig::default()
        };
        let environment = SyntheticEnvironment::demo(start(), synthetic.clone());
        let universe = crate::reference::synthetic_universe(&environment, start())?;
        // The premise, part one: the derived universe is populated, and by
        // exactly the instruments the feed stamps its bars with.
        assert!(
            !universe.is_empty(),
            "the derived universe is empty, which is the defect itself"
        );
        assert_eq!(
            universe.len(),
            environment.instruments().len(),
            "the universe does not cover the feed's instrument list"
        );

        let round = |universe: Universe| -> Result<RoundSummary> {
            let mut platform = platform()?;
            let mut engine = EvolutionEngine::new(
                EvolutionConfig {
                    every_cycles: 1,
                    ..EvolutionConfig::default()
                },
                Box::new(SyntheticEnvironment::demo(start(), synthetic.clone())),
                7,
                universe,
            )?;
            let mut now = start();
            for _ in 0..((engine.config.minimum_bars as i64 + 30) * 2) {
                now = now.saturating_add(Duration::from_mins(1));
                engine.sense(&mut platform, now)?;
            }
            engine
                .maybe_turn(&mut platform, 1, now)?
                .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))
        };

        // The premise, part two: over an empty universe the same search
        // proposes candidates and registers none of them. Without this half,
        // "registered >= 1" below could be true of a gate that admits
        // everything regardless of whether anything filled.
        let starved = round(Universe::new())?;
        assert!(
            starved.proposed >= 1,
            "the search proposed nothing, so the contrast is about the search: {starved:?}"
        );
        assert_eq!(
            starved.registered, 0,
            "a candidate registered against an empty universe; no order can have filled,              so its evidence is a flat line: {starved:?}"
        );
        assert!(
            starved.discarded >= 1,
            "nothing was discarded over the empty universe: {starved:?}"
        );

        let fed = round(universe)?;
        assert!(
            fed.registered >= 1,
            "no candidate registered against the populated universe: {fed:?}. Registration              requires `evaluate` to return evidence, and `evaluate` refuses a run with no              fills -- so this also asserts the backtests actually traded"
        );
        Ok(())
    }

    #[test]
    fn the_first_round_crowns_a_champion_from_a_candidate_the_gate_admitted() -> Result<()> {
        // Before this module nothing was ever named the strategy that speaks
        // for an instrument, so nothing could ever be replaced and the whole
        // champion/challenger apparatus was unreachable from any deployed
        // path.
        let (_platform, engine, summaries) = run_rounds(1)?;
        let first = summaries
            .first()
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;
        // The premise: the gate admitted something. A round that registered
        // nothing has no candidate to crown and would pass this vacuously.
        assert!(
            first.registered >= 1,
            "no candidate was registered, so the crowning is untested: {first:?}"
        );
        let challenge = first
            .challenge
            .as_ref()
            .ok_or_else(|| Error::not_found("the contest that follows registration"))?;
        assert!(
            challenge.crowned.is_some(),
            "the first round installed no champion: {:?}",
            challenge.refusals
        );
        assert_eq!(engine.succession_stats().installations, 1);
        assert_eq!(engine.desk.champions(), 1);
        // And a bootstrap is not a comparison: nothing was minted or judged.
        assert_eq!(challenge.minted, 0);
        assert_eq!(engine.succession_stats().comparisons, 0);
        Ok(())
    }

    #[test]
    fn a_champion_the_ledger_has_not_demoted_survives_into_the_next_round() -> Result<()> {
        // Written against a real regression. The desk first used
        // `ChampionBook::stale`, which returns champions whose stage no longer
        // holds capital -- true of every candidate-stage champion, because a
        // bottom-rung strategy has never held capital. It dethroned the
        // incumbent every round: three rounds produced three installations,
        // zero challenges, and a contest that could never run.
        let (_platform, engine, summaries) = run_rounds(3)?;
        assert!(
            summaries.len() >= 3,
            "the premise failed: only {} round(s) ran",
            summaries.len()
        );
        assert_eq!(
            engine.succession_stats().installations,
            1,
            "the champion was reinstalled, so it did not survive its round"
        );
        assert!(
            engine.succession_stats().challenges >= 2,
            "no challenge round ran after the bootstrap: {:?}",
            engine.succession_stats()
        );
        Ok(())
    }

    #[test]
    fn every_challenger_minted_is_counted_as_a_trial() -> Result<()> {
        // The multiple-comparisons invariant. Minting challengers from the
        // champion and reporting the best one, deflated only by the generative
        // search that produced their parent, is the problem the trial ledger
        // exists to prevent -- with extra steps.
        let (_platform, engine, summaries) = run_rounds(2)?;
        let (first, second) = match summaries.as_slice() {
            [first, second, ..] => (first, second),
            other => {
                return Err(Error::not_found(format!("two rounds; got {}", other.len())));
            }
        };
        let challenge = second
            .challenge
            .as_ref()
            .ok_or_else(|| Error::not_found("the second round's contest"))?;
        // The premise: challengers were actually minted. With none, the trial
        // arithmetic below would hold for the wrong reason.
        assert!(
            challenge.minted > 0,
            "no challenger was minted, so the count is untested: {challenge:?}"
        );
        assert_eq!(
            second.trials - first.trials,
            engine.config.candidates + challenge.minted,
            "the ledger grew by the generative search alone; the {} challenger(s) minted \
             from the champion were not counted",
            challenge.minted
        );
        assert_eq!(engine.succession_stats().challenges, 1);
        Ok(())
    }

    #[test]
    fn an_instrument_nothing_traded_is_not_searched_even_when_it_is_the_only_candidate()
    -> Result<()> {
        // The ranking already puts an untradeable instrument last, so this is
        // the case only the filter can decide: it is the sole subject with
        // enough history, and the round must refuse rather than search it.
        // Every order would be rejected by the impact model, the equity curve
        // would be flat, and the trials would be spent on a question with no
        // answer.
        let mut platform = platform()?;
        let mut engine = engine(1);
        let mut now = start();
        for _ in 0..((engine.config.minimum_bars as i64 + 30) * 2) {
            now = now.saturating_add(Duration::from_mins(1));
            engine.sense(&mut platform, now)?;
        }

        let untradeable = "OBJ000000000000000UST10Y";
        // Real bars from the illiquid bond, with every traded volume set to
        // zero: an instrument nothing traded on, which is the case under test.
        //
        // The bond's own volumes were used until 2026-09-08, on the strength
        // of its being thin enough that `tradeable_capital` refused it. That
        // held only for the short window the engine then searched on. Once a
        // search waited for the history its gate requires, the same bond
        // accumulated enough traded notional to be backtestable and the
        // premise failed — the test had been resting on how little data the
        // engine happened to collect rather than on anything about the
        // instrument. Zeroing the volumes makes the premise structural, and
        // true at any window length.
        let bars: Vec<Bar> = engine
            .history
            .get(untradeable)
            .cloned()
            .ok_or_else(|| Error::not_found("the bond's own history"))?
            .into_iter()
            .map(|mut bar| {
                bar.volume = Decimal::ZERO;
                bar
            })
            .collect();
        // The premise, in two parts: it has enough history to be eligible, and
        // it genuinely cannot be traded.
        assert!(
            bars.len() >= engine.config.minimum_bars,
            "the premise failed: too little history to be eligible anyway"
        );
        assert!(
            tradeable_capital(&bars, engine.config.max_weight).is_err(),
            "the premise failed: a series with no traded volume was accepted as backtestable, \
             so this proves nothing"
        );

        engine.history.clear();
        engine.history.insert(untradeable.to_string(), bars);
        assert!(
            engine.maybe_turn(&mut platform, 1, now)?.is_none(),
            "the round searched an instrument on which nothing traded"
        );
        // And the trial ledger did not move: a refused round is not a search.
        assert_eq!(platform.central().factory().candidates().count(), 0);
        Ok(())
    }

    #[test]
    fn a_run_in_which_nothing_traded_is_refused_rather_than_scored() -> Result<()> {
        // The failure this whole change exists for. Before it, an empty
        // universe and a day-long decision lag on minute bars meant every
        // backtest returned a perfectly flat equity curve, and the holdout
        // gate scored that flat line as a Sharpe ratio.
        let (_platform, engine, summaries) = run_rounds(1)?;
        let subject = ObjectId::from_string(
            summaries
                .first()
                .map(|summary| summary.subject.clone())
                .ok_or_else(|| Error::not_found("a round"))?,
        );
        let bars = engine
            .history
            .get(subject.as_str())
            .cloned()
            .ok_or_else(|| Error::not_found("the subject's own history"))?;
        // The premise: this instrument is tradeable, so nothing about the data
        // is what stops the run. Only the strategy is.
        assert!(
            tradeable_capital(&bars, engine.config.max_weight).is_ok(),
            "the premise failed: the subject cannot be backtested at all"
        );

        // A strategy that compiles, reads a real feature, fires on every bar
        // and stands down every time. It is the shape every generated
        // candidate had before the grammar fix, and it must not be scoreable.
        let spec = StrategySpec::new(
            qip_contracts::StrategyId::new("always-stands"),
            subject.clone(),
            Duration::from_secs(60),
        )
        .with_rule(Rule::new(
            "stand",
            SignalKind::Stand,
            Expr::feature(FeatureKey::new("up_bar", subject.clone())),
            Expr::Exact(qip_core::Decimal::ONE),
            Expr::Statistic(0.5),
            100,
        ));

        let error = match engine.evaluate(&subject, &spec, &bars) {
            Err(error) => error,
            Ok(_) => {
                return Err(Error::invalid(
                    "a strategy that never took a position was scored as evidence",
                ));
            }
        };
        assert!(
            error.message().contains("is not evidence"),
            "the refusal does not name the cause: {}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn the_deeper_instrument_is_searched_even_when_a_thinner_one_has_more_history() -> Result<()> {
        // Ranking by bar count is really ranking by key order on this feed:
        // every subject accumulates bars at the same rate, so the tie is broken
        // by name. That is how the synthetic exchange's illiquid government
        // bond -- which sorts last -- came to be the subject of every round
        // this node had ever run.
        //
        // Both subjects below are tradeable, so the depth *filter* cannot
        // decide this; only the ranking can.
        let mut platform = platform()?;
        let mut engine = engine(1);
        let mut now = start();
        for _ in 0..((engine.config.minimum_bars as i64 + 30) * 2) {
            now = now.saturating_add(Duration::from_mins(1));
            engine.sense(&mut platform, now)?;
        }

        let deep = "OBJ00000000000000000VNTG";
        let thin = "OBJ00000000000000000ATFB";
        let mut deep_bars = engine
            .history
            .get(deep)
            .cloned()
            .ok_or_else(|| Error::not_found("the deep instrument's history"))?;
        let thin_bars = engine
            .history
            .get(thin)
            .cloned()
            .ok_or_else(|| Error::not_found("the thin instrument's history"))?;

        // Give the deeper instrument *less* history, so bar count and depth
        // point at different subjects and only one of them can be deciding.
        deep_bars.truncate(engine.config.minimum_bars + 1);
        let deep_depth = depth(&deep_bars).ok_or_else(|| Error::not_found("the deep depth"))?;
        let thin_depth = depth(&thin_bars).ok_or_else(|| Error::not_found("the thin depth"))?;
        assert!(
            deep_depth > thin_depth,
            "the premise failed: {deep} is not the deeper of the two"
        );
        assert!(
            deep_bars.len() < thin_bars.len(),
            "the premise failed: the deeper instrument does not have less history"
        );
        assert!(
            tradeable_capital(&thin_bars, engine.config.max_weight).is_ok(),
            "the premise failed: the thinner instrument is filtered out anyway, so the \
             ranking is not what decides"
        );

        engine.history.clear();
        engine.history.insert(deep.to_string(), deep_bars);
        engine.history.insert(thin.to_string(), thin_bars);

        let summary = engine
            .maybe_turn(&mut platform, 1, now)?
            .ok_or_else(|| Error::not_found("a round over two eligible subjects"))?;
        assert_eq!(
            summary.subject, deep,
            "the round searched the instrument with more bars rather than the one it can \
             most readily trade"
        );
        Ok(())
    }

    #[test]
    fn a_challenger_derived_from_a_different_champion_is_not_judged_against_this_one() -> Result<()>
    {
        // A verdict is about a pair. Judging a challenger that mutated some
        // other parent would report a comparison that was never made, and the
        // succession it could produce would crown a strategy against a champion
        // it had never faced.
        let subject = ObjectId::from_string("OBJ00000000000000000VNTG");
        let mut foundry = StrategyFoundry::new(
            bar_catalogue(&subject)?,
            Grammar::over(FeaturePalette::from_catalogue(
                &bar_catalogue(&subject)?,
                &subject,
            )?),
            "central-research",
            qip_contracts::venue::VenueId::new("XSIM"),
            "test",
            11,
        )?;
        foundry.search(4)?;
        let candidates: Vec<Candidate> = foundry.pending().to_vec();
        // The premise: two *distinct* candidates, so "derived from a different
        // champion" is a real difference and not the same strategy twice.
        assert!(
            candidates.len() >= 2,
            "the search produced {} candidate(s); two are needed",
            candidates.len()
        );
        assert_ne!(candidates[0].id(), candidates[1].id());

        let ledger = LifecycleLedger::new();
        let mut here = SuccessionDesk::new(2, ChallengerPolicy::default())?;
        let mut elsewhere = SuccessionDesk::new(2, ChallengerPolicy::default())?;
        here.install_first(&ledger, &subject, &candidates[0], start())?;
        elsewhere.install_first(&ledger, &subject, &candidates[1], start())?;

        let mut compiler = StrategyCompiler::new(bar_catalogue(&subject)?);
        let grammar = Grammar::over(FeaturePalette::from_catalogue(
            &bar_catalogue(&subject)?,
            &subject,
        )?);
        let run = elsewhere
            .mint(subject.as_str(), &mut foundry, &mut compiler, grammar, 13)
            .ok_or_else(|| Error::not_found("a challenge round from the other champion"))?;
        let challenger = run
            .accepted()
            .first()
            .ok_or_else(|| Error::not_found("an accepted challenger"))?;
        // The premise: this challenger really did mutate the other champion.
        assert_eq!(challenger.champion(), candidates[1].id());

        let returns = vec![0.001_f64; 300];
        let error = match here.judge(
            &ledger,
            &subject,
            challenger,
            NetReturns::flat_bps(&returns, 5.0)?,
            NetReturns::flat_bps(&returns, 5.0)?,
            PERIODS_PER_YEAR,
            &foundry,
            "trending/normal",
            start(),
        ) {
            Err(error) => error,
            Ok(_) => {
                return Err(Error::invalid(
                    "a challenger of another champion was judged against this one",
                ));
            }
        };
        assert!(
            error.message().contains("mutated a different parent"),
            "the refusal does not name the cause: {}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn the_node_fits_and_registers_a_model_rather_than_only_being_able_to() -> Result<()> {
        // The defect this closes was not that the model machinery was wrong.
        // It was that no running process built a registry, fitted anything, or
        // called `register_fit` -- so `drift_score` was 0.0 on every card that
        // could exist and the drift branch of `decision_eligibility` could not
        // fire. A desk that compiles and is never called is the same gap in a
        // new place, so this test drives it through the engine.
        let mut platform = platform()?;
        let mut engine = engine(1);
        let mut now = start();
        for _ in 0..((engine.config.minimum_bars as i64 + 30) * 6) {
            now = now.saturating_add(Duration::from_mins(1));
            engine.sense(&mut platform, now)?;
        }
        // The premise: nothing has been fitted, and the cadence is about to
        // permit one.
        assert_eq!(engine.learning_stats().rounds, 0);

        let mut round = None;
        for cycle in 1..=16u64 {
            if let Some(produced) = engine.maybe_learn(cycle, now)? {
                round = Some(produced);
                break;
            }
        }
        let round = round.ok_or_else(|| {
            Error::not_found("a learning round within sixteen cycles of the default cadence")
        })?;
        assert!(
            round.registration.is_some(),
            "the round fitted nothing: {:?}",
            round.ineligible
        );
        assert!(
            engine.learning_stats().registered + engine.learning_stats().without_skill >= 1,
            "a model was registered without the desk counting it"
        );
        Ok(())
    }

    #[test]
    fn capital_is_sized_so_a_full_weight_order_clears_the_participation_limit() -> Result<()> {
        // Ten million against a one-minute bar is an order worth three
        // thousand times the volume traded in the window, and the impact model
        // refuses it -- so every order was rejected and the curve was flat.
        let (_platform, engine, _) = run_rounds(1)?;
        let bars = engine
            .history
            .values()
            .find(|bars| tradeable_capital(bars, engine.config.max_weight).is_ok())
            .ok_or_else(|| Error::not_found("a subject that traded"))?;
        let capital = tradeable_capital(bars, engine.config.max_weight)?;
        // The premise: the institutional default is the thing being replaced.
        let default = BacktestConfig::default().initial_capital;
        assert!(
            capital < default,
            "the sized capital {capital} is not below the {default} default, so nothing changed"
        );
        // A full-weight order must sit inside the impact model's own ceiling
        // on a bar of ordinary size.
        let mut notionals: Vec<f64> = bars
            .iter()
            .map(|bar| bar.volume.to_f64() * bar.close.to_f64())
            .filter(|notional| *notional > 0.0)
            .collect();
        notionals.sort_by(f64::total_cmp);
        let typical = notionals[notionals.len() / 2];
        let order = capital.to_f64() * engine.config.max_weight;
        assert!(
            order < typical * 0.20,
            "a full-weight order of {order} is above 20% of a typical bar's {typical}"
        );
        Ok(())
    }

    #[test]
    fn a_turn_registers_scored_candidates_with_the_platform_factory() -> Result<()> {
        let mut platform = platform()?;
        let mut engine = engine(1);

        // The premise, in three parts: the factory holds nothing, the engine
        // has sensed nothing, and a turn before any history refuses to run
        // rather than searching on air.
        assert_eq!(platform.central().factory().candidates().count(), 0);
        assert!(engine.maybe_turn(&mut platform, 1, start())?.is_none());

        // Sense enough minutes that at least one subject crosses the
        // minimum-history bar. The adapter is the same synthetic exchange the
        // node runs, stepped on its own interval.
        let mut now = start();
        let minutes = (engine.config.minimum_bars as i64 + 30) * 2;
        let mut observed = 0usize;
        for _ in 0..minutes {
            now = now.saturating_add(Duration::from_mins(1));
            observed += engine.sense(&mut platform, now)?;
        }
        assert!(observed > 0, "the synthetic exchange fed nothing at all");
        assert!(
            engine
                .history
                .values()
                .any(|bars| bars.len() >= engine.config.minimum_bars),
            "no subject accumulated the {} bars a search needs; the premise fails before \
             the turn is even attempted",
            engine.config.minimum_bars
        );

        let summary = engine
            .maybe_turn(&mut platform, 1, now)?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;

        assert_eq!(summary.proposed, engine.config.candidates);
        assert_eq!(
            summary.registered + summary.discarded,
            summary.admitted,
            "candidates went missing between scoring and the ledger: {summary:?}"
        );
        assert!(
            summary.trials >= summary.admitted,
            "the trial count does not cover the round that just ran"
        );
        assert_eq!(
            platform.central().factory().candidates().count(),
            summary.registered,
            "the factory holds a different number of candidates than the round registered"
        );
        assert!(
            summary.registered >= 1,
            "every scored candidate was discarded ({summary:?}); if the grammar or seed \
             changed, pick a seed where at least one candidate survives, because a test \
             that always registers zero is not exercising registration"
        );

        // And none of them hold capital: an automated search must land on the
        // bottom rung, never above it.
        for candidate in platform.central().factory().candidates() {
            assert!(
                !platform
                    .central()
                    .factory()
                    .holds_capital(candidate.strategy()),
                "{} entered the ladder holding capital",
                candidate.strategy()
            );
        }
        Ok(())
    }

    #[test]
    fn every_candidate_the_round_registers_is_put_in_front_of_the_holdout_gate() -> Result<()> {
        // The defect this pins: the loop searched, scored and registered
        // candidates carrying real holdout evidence, and then never asked the
        // gate about any of them. `promote` had no caller outside tests, so
        // every candidate stayed a `Candidate`, no baseline was written, and
        // the retirement machinery that skips a strategy without a baseline
        // had nothing to observe in any deployment.
        //
        // The assertion is the accounting rather than a pass count, and that
        // is deliberate: whether this seed's candidates clear the holdout gate
        // depends on the grammar and the synthetic tape, and a test demanding
        // a pass would break for a reason that is not a defect. What must
        // never happen again is a registered candidate nobody asked about.
        let mut platform = platform()?;
        let mut engine = engine(1);

        let mut now = start();
        let minutes = (engine.config.minimum_bars as i64 + 30) * 2;
        for _ in 0..minutes {
            now = now.saturating_add(Duration::from_mins(1));
            engine.sense(&mut platform, now)?;
        }
        let summary = engine
            .maybe_turn(&mut platform, 1, now)?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;

        // Premise: something was registered. Without this the accounting below
        // is 0 == 0 + 0 and would hold over a loop that does nothing at all.
        assert!(
            summary.registered >= 1,
            "nothing was registered, so this test cannot show anything was judged: {summary:?}"
        );
        assert_eq!(
            summary.promoted + summary.gate_refused,
            summary.registered,
            "{} candidate(s) were registered and {} were put in front of the gate; the \
             difference is evidence gathered and never judged: {summary:?}",
            summary.registered,
            summary.promoted + summary.gate_refused
        );

        // Whatever the gate decided, the ladder agrees with the count: a
        // promotion that was counted and did not move the strategy would be a
        // number in a summary rather than a fact about the ledger.
        let moved = platform
            .central()
            .factory()
            .candidates()
            .filter(|candidate| {
                platform.central().factory().stage_of(candidate.strategy())
                    != qip_contracts::gate::GateStage::Candidate
            })
            .count();
        assert_eq!(
            moved, summary.promoted,
            "the summary counted {} promotion(s) and {moved} strategy(ies) left Candidate",
            summary.promoted
        );

        // And none of them reached a rung that holds capital: the two rungs
        // that do require a dual human approval, and this loop passes none.
        for candidate in platform.central().factory().candidates() {
            assert!(
                !platform
                    .central()
                    .factory()
                    .holds_capital(candidate.strategy()),
                "{} reached a capital-holding rung from an automated search",
                candidate.strategy()
            );
        }
        Ok(())
    }

    #[test]
    fn a_search_never_runs_on_less_history_than_its_gate_will_accept() {
        // The defect: the engine searched from about 188 bars, which holds out
        // 47 against the holdout gate's minimum of 250, so every round before
        // roughly a thousand bars registered candidates the gate could not
        // admit whatever they contained. That was not merely wasted work —
        // each one is charged to the family's cumulative trial count, and the
        // count deflates the Sharpe every later candidate is judged against.
        // The futile rounds were raising the bar for the rounds that could
        // have succeeded.
        let policy = qip_lifecycle::gates::HoldoutPolicy::default();
        let fraction = EvolutionConfig::default().holdout_fraction;
        let minimum = EvolutionConfig::default().minimum_bars;
        let held_out = (minimum as f64 * fraction).floor() as usize;
        assert!(
            held_out >= policy.minimum_observations,
            "a search may start at {minimum} bars, which holds out {held_out} against the \
             gate's minimum of {}; every candidate from such a round is refused on sample \
             adequacy alone and still charged as a trial",
            policy.minimum_observations
        );
        // Derived, not written down twice: raising the gate's minimum must
        // raise this with it, so the premise is that the helper tracks the
        // policy rather than that some constant happens to be large enough.
        assert!(
            minimum_bars_for_the_holdout_gate(fraction) >= minimum_bars_for_the_holdout_gate(0.5),
            "a smaller holdout fraction needs more bars, not fewer; the derivation is inverted"
        );
    }

    #[test]
    fn the_folds_a_round_reports_are_the_ones_the_gate_rebuilds() -> Result<()> {
        // The evaluation used to build its folds by hand and count the purge
        // itself, and its arithmetic disagreed with `PurgedSplit` — it charged
        // a fold's trailing label horizon as purged on top of the leading
        // purge, reporting four where the splitter found two. The gate
        // rebuilds the folds from the run's own description and compares, so
        // every candidate was refused on `purging_and_embargo_applied`
        // regardless of merit. The gate was right; there were two
        // implementations of one split.
        //
        // This asserts the outcome an operator would see: whatever the gate
        // decides about a candidate's *performance*, it no longer decides
        // against it because the folds cannot be rebuilt.
        let mut platform = platform()?;
        let mut engine = engine(1);
        let mut now = start();
        let minutes = (engine.config.minimum_bars as i64 + 60) * 2;
        for _ in 0..minutes {
            now = now.saturating_add(Duration::from_mins(1));
            engine.sense(&mut platform, now)?;
        }
        let summary = engine
            .maybe_turn(&mut platform, 1, now)?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;
        assert!(
            summary.registered >= 1,
            "nothing was registered, so no candidate reached the gate: {summary:?}"
        );
        // Premise: the gate did speak. Without a refusal to read, a test that
        // looks for a phrase in the refusals passes over an empty list.
        assert!(
            !summary.gate_refusals.is_empty() || summary.promoted > 0,
            "the gate neither admitted nor refused anything: {summary:?}"
        );
        for refusal in &summary.gate_refusals {
            assert!(
                !refusal.contains("purging_and_embargo_applied"),
                "a candidate was refused because its folds could not be rebuilt from its own \
                 description: {refusal}"
            );
            assert!(
                !refusal.contains("holdout_sample_adequate"),
                "a candidate was refused for too small a sample, which means the search ran \
                 before it had the history its gate requires: {refusal}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_disabled_engine_never_turns_and_a_cadence_skips_off_cycles() -> Result<()> {
        let mut platform = platform()?;
        let mut engine = engine(0);
        let mut now = start();
        for _ in 0..8 {
            now = now.saturating_add(Duration::from_mins(1));
            engine.sense(&mut platform, now)?;
        }
        assert!(engine.maybe_turn(&mut platform, 4, now)?.is_none());

        let mut engine = engine_with_every(3);
        assert!(engine.maybe_turn(&mut platform, 1, now)?.is_none());
        assert!(engine.maybe_turn(&mut platform, 2, now)?.is_none());
        Ok(())
    }

    fn engine_with_every(every: u64) -> EvolutionEngine {
        engine(every)
    }
}
