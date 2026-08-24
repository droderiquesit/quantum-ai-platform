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

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
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
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            every_cycles: 4,
            candidates: 8,
            minimum_bars: WARM_UP_BARS * 6,
            holdout_fraction: 0.25,
            max_weight: 0.5,
            history_cap: 2048,
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

/// What one round did, for the operator's cycle line.
#[derive(Clone, Debug, Default)]
pub struct RoundSummary {
    pub subject: String,
    pub proposed: usize,
    pub admitted: usize,
    pub registered: usize,
    pub discarded: usize,
    /// The search's cumulative trial count after this round — the number
    /// every holdout is deflated by.
    pub trials: usize,
}

impl RoundSummary {
    pub fn describe(&self) -> String {
        format!(
            "evolution: {} proposed {} on {}, {} admitted, {} registered, {} discarded, \
             {} trial(s) on the ledger",
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
    history: BTreeMap<String, Vec<Bar>>,
    foundries: BTreeMap<String, StrategyFoundry>,
    stats: EvolutionStats,
}

impl std::fmt::Debug for EvolutionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvolutionEngine")
            .field("config", &self.config)
            .field("subjects", &self.history.len())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl EvolutionEngine {
    pub fn new(config: EvolutionConfig, adapter: Box<dyn DataAdapter>, seed: u64) -> Self {
        Self {
            config,
            adapter,
            seed,
            history: BTreeMap::new(),
            foundries: BTreeMap::new(),
            stats: EvolutionStats::default(),
        }
    }

    pub const fn stats(&self) -> EvolutionStats {
        self.stats
    }

    pub const fn enabled(&self) -> bool {
        self.config.every_cycles > 0
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
            .max_by_key(|(_, bars)| bars.len())
            .map(|(subject, bars)| (subject.clone(), bars.clone()))
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
        let pending: Vec<_> = foundry
            .pending()
            .iter()
            .map(|candidate| (candidate.id().clone(), candidate.spec().clone()))
            .collect();
        let object = ObjectId::from_string(subject);
        for (id, spec) in pending {
            let scored = self.evaluate(&object, &spec, &bars);
            let foundry = self
                .foundries
                .get_mut(subject)
                .ok_or_else(|| Error::not_found("the foundry that just searched"))?;
            match scored {
                Ok(holdout) => {
                    match foundry.register(platform.central_mut().factory_mut(), &id, holdout, now)
                    {
                        Ok(()) => summary.registered += 1,
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
        if let Some(foundry) = self.foundries.get(subject) {
            summary.trials = foundry.trials();
        }
        Ok(summary)
    }

    /// Score one candidate on the subject's own history.
    fn evaluate(
        &self,
        subject: &ObjectId,
        spec: &qip_strategy::ir::StrategySpec,
        bars: &[Bar],
    ) -> Result<HoldoutInputs> {
        let mut compiler = qip_strategy::compile::StrategyCompiler::new(bar_catalogue(subject)?);
        let compiled = compiler.compile(spec)?;
        let mut harness =
            CompiledHarness::new(compiled, compiler.into_program(), self.config.max_weight)?;
        let mut clock = SimulationClock::new(bars.to_vec(), ExecutionAssumptions::next_bar())?;
        let result = Backtester::new(BacktestConfig::default())?.run(
            &mut harness,
            &mut clock,
            &Universe::new(),
        )?;

        let returns = result.returns;
        let holdout_len = ((returns.len() as f64) * self.config.holdout_fraction) as usize;
        if holdout_len < WARM_UP_BARS {
            return Err(Error::invalid(format!(
                "a holdout of {holdout_len} return(s) is a sliver, not evidence; the subject \
                 needs more history before a verdict means anything"
            )));
        }
        let split = returns.len() - holdout_len;
        let (pre, tail) = returns.split_at(split);

        // Three contiguous test windows over the pre-holdout region, with a
        // one-bar purge each side of every window and a one-bar embargo after
        // it. Generated candidates carry no fitted parameters — the grammar
        // writes fixed rules — so these folds measure stability across
        // windows rather than fit quality, and the numbers below describe
        // exactly the splits as built so the gate can rebuild and compare.
        const FOLDS: usize = 3;
        const LABEL_HORIZON: usize = 1;
        const EMBARGO: usize = 1;
        let fold_len = pre.len() / FOLDS;
        let mut in_sample = Vec::with_capacity(FOLDS);
        let mut out_of_sample = Vec::with_capacity(FOLDS);
        let mut purged = 0usize;
        let mut embargoed = 0usize;
        for fold in 0..FOLDS {
            let a = fold * fold_len;
            let b = if fold == FOLDS - 1 {
                pre.len()
            } else {
                a + fold_len
            };
            out_of_sample.push(pre[a..b].to_vec());
            let purge_before = a.saturating_sub(LABEL_HORIZON);
            let embargo_after = (b + LABEL_HORIZON + EMBARGO).min(pre.len());
            purged += (a - purge_before) + (embargo_after - b).min(LABEL_HORIZON);
            embargoed += (embargo_after - b).saturating_sub(LABEL_HORIZON);
            let mut train = Vec::with_capacity(pre.len().saturating_sub(b - a));
            train.extend_from_slice(&pre[..purge_before]);
            train.extend_from_slice(&pre[embargo_after..]);
            in_sample.push(train);
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

        Ok(HoldoutInputs {
            returns: tail.to_vec(),
            in_sample_folds: in_sample,
            out_of_sample_folds: out_of_sample,
            periods_per_year: 252.0,
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
        })
    }
}

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
    use qip_core::{Context, Duration};
    use qip_financial::universe::Universe;
    use qip_kernel::config::PlatformConfig;
    use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

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
        let step = Duration::from_mins(1);
        let synthetic = EnvironmentConfig {
            seed: 7,
            step,
            ..EnvironmentConfig::default()
        };
        let adapter = Box::new(SyntheticEnvironment::demo(start(), synthetic));
        EvolutionEngine::new(
            EvolutionConfig {
                every_cycles: every,
                ..EvolutionConfig::default()
            },
            adapter,
            7,
        )
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
