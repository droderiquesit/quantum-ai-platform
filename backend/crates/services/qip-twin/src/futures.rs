//! Synthetic future trees over a compact twin state (R12-TICK-7199;
//! §9.9, R12-TICK-3096–3100).
//!
//! The twin could replay what happened and price what the platform declined.
//! It could not explore a world that is **not** in history — a shock larger
//! than any it recorded, a propagation stronger than the graph has seen —
//! and score that exploration against what then happened. This module does,
//! and it is built around one rule above the others.
//!
//! # A synthetic branch is never observed data
//!
//! Every [`Branch`] carries [`Provenance::Synthetic`], an enum with exactly
//! one variant, so a branch cannot be serialised claiming to be observed and
//! a document claiming `"observed"` does not deserialise as a branch. The
//! impact a branch carries is a hypothesis about a world; nothing converts
//! it into a price, a tick, a bar or a fill, and nothing in this crate
//! accepts a branch where an observation is expected. Reality enters only
//! through [`FutureTree::observe`], as an [`ObservedMoves`] value, and only
//! ever reweights branches — it is never written into one.
//!
//! # A twin is a compact learned state, not a copy of the world
//!
//! [`TwinState::fit`] reduces each instrument's history to a drift, a
//! volatility and the last mid it saw, plus each instrument's beta to the
//! common move. Its size depends on the number of instruments and not on how
//! much history it was fitted from, and it is fitted only from observations
//! at or before its `as_of` — a twin fitted on prices it had not yet seen
//! would grow futures that already know the answer.
//!
//! # Branches vary the causal assumptions, and reality prunes them
//!
//! [`FutureTree::grow`] crosses every world driver with a grid of shock sizes
//! (in standard deviations, deliberately including sizes beyond anything in
//! the fitted history), propagation strengths and signs. Each branch carries
//! the assumptions that made it, the evidence the driver cites, the
//! propagation path it assumed, its per-instrument impact, a hedge, and a
//! prior probability. [`FutureTree::observe`] scores the tree by the log
//! probability it gave to what happened, reweights every live branch by its
//! likelihood, prunes the ones reality made implausible, and — when reality
//! lands outside every live branch — splits a new branch at what happened,
//! so the tree's next exploration starts from the world it is actually in.
//!
//! Everything is deterministic: no clock is read and no ambient randomness
//! drawn. The same state, drivers and configuration grow the same tree.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a branch is. One variant, on purpose: see the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Synthetic,
}

/// One instrument as the twin learned it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstrumentDynamics {
    /// Mean log return per second, in basis points.
    pub drift_bps_per_sec: f64,
    /// Standard deviation of log return per square-root second, in bps.
    pub vol_bps_per_sqrt_sec: f64,
    /// Beta of the instrument's returns to the cross-sectional mean return.
    pub beta: f64,
    /// The last mid the twin saw, at or before `as_of`.
    pub last_mid: f64,
    pub observations: usize,
}

/// The compact learned state a tree is grown from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TwinState {
    pub as_of: Timestamp,
    pub instruments: BTreeMap<String, InstrumentDynamics>,
}

impl TwinState {
    /// Fit from mid series, reading only points at or before `as_of`.
    /// Instruments with fewer than three such points are left out rather
    /// than given a volatility of nothing. Refuses a fit that leaves no
    /// instrument.
    pub fn fit(series: &BTreeMap<String, Vec<(Timestamp, f64)>>, as_of: Timestamp) -> Result<Self> {
        // Per-instrument returns, with their interval, from knowable points.
        let mut returns: BTreeMap<&str, Vec<(Timestamp, f64, f64)>> = BTreeMap::new();
        let mut last: BTreeMap<&str, f64> = BTreeMap::new();
        for (instrument, points) in series {
            let mut known: Vec<&(Timestamp, f64)> = points
                .iter()
                .filter(|(at, mid)| *at <= as_of && mid.is_finite() && *mid > 0.0)
                .collect();
            known.sort_by_key(|(at, _)| *at);
            if known.len() < 3 {
                continue;
            }
            let mut rs = Vec::new();
            for pair in known.windows(2) {
                let dt = pair[1].0.since(pair[0].0).as_secs_f64();
                if dt > 0.0 {
                    rs.push((pair[1].0, (pair[1].1 / pair[0].1).ln() * 10_000.0, dt));
                }
            }
            if rs.len() < 2 {
                continue;
            }
            last.insert(instrument.as_str(), known[known.len() - 1].1);
            returns.insert(instrument.as_str(), rs);
        }
        if returns.is_empty() {
            return Err(Error::invalid(format!(
                "no instrument has three knowable mids at or before {as_of}; a twin fitted from \
                 nothing would grow futures from nothing"
            )));
        }
        // The common move per instant: the mean return of every instrument
        // that moved at it, for the betas.
        let mut common: BTreeMap<Timestamp, (f64, usize)> = BTreeMap::new();
        for rs in returns.values() {
            for (at, r, _) in rs {
                let entry = common.entry(*at).or_insert((0.0, 0));
                entry.0 += r;
                entry.1 += 1;
            }
        }
        let mut instruments = BTreeMap::new();
        for (instrument, rs) in &returns {
            let total_time: f64 = rs.iter().map(|(_, _, dt)| dt).sum();
            let total_return: f64 = rs.iter().map(|(_, r, _)| r).sum();
            let drift = total_return / total_time;
            let variance: f64 = rs
                .iter()
                .map(|(_, r, dt)| (r - drift * dt).powi(2) / dt)
                .sum::<f64>()
                / rs.len() as f64;
            let (mut cov, mut var_m) = (0.0, 0.0);
            for (at, r, _) in rs {
                if let Some((sum, n)) = common.get(at) {
                    let m = sum / *n as f64;
                    cov += r * m;
                    var_m += m * m;
                }
            }
            let beta = if var_m > 1e-18 { cov / var_m } else { 1.0 };
            instruments.insert(
                (*instrument).to_string(),
                InstrumentDynamics {
                    drift_bps_per_sec: drift,
                    vol_bps_per_sqrt_sec: variance.sqrt(),
                    beta,
                    last_mid: last.get(instrument).copied().unwrap_or(0.0),
                    observations: rs.len() + 1,
                },
            );
        }
        Ok(Self { as_of, instruments })
    }

    fn digest(&self) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(self)?))
    }
}

/// A world driver the tree explores: an event or entity, the instruments it
/// reaches with a sensitivity each, and the evidence it rests on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldDriver {
    pub id: String,
    /// The scenario family the driver's branches belong to (R12-WORLD-9035):
    /// the class of world it explores, such as `guidance` or
    /// `monetary_policy`. Two drivers of one family are alternatives of one
    /// scenario and are weighed against each other in
    /// [`FutureTree::families`]; a driver of another family is a different
    /// question, and mixing the two into one distribution lets a family with
    /// many drivers crowd out one with few. `None` puts the driver in a
    /// family of its own, named by its id. An empty name, or one of the two
    /// the tree reserves ([`BASELINE_FAMILY`], [`REALITY_FAMILY`]), is
    /// refused by [`FutureTree::grow`] rather than merged into the baseline.
    #[serde(default)]
    pub family: Option<String>,
    /// Sensitivity of each instrument to one standard-deviation shock of the
    /// driver, as a multiple of the instrument's own volatility.
    pub sensitivities: BTreeMap<String, f64>,
    pub evidence: Vec<String>,
}

/// The family of the baseline branch: the world continuing on its drift,
/// which no driver explores and so belongs to no driver's family.
pub const BASELINE_FAMILY: &str = "baseline";

/// The family of a branch split from reality with no live branch near enough
/// to inherit a family from.
pub const REALITY_FAMILY: &str = "reality";

impl WorldDriver {
    /// The family this driver's branches carry. Refuses an empty name and a
    /// reserved one, so a driver cannot pose as the baseline.
    fn family(&self) -> Result<String> {
        let family = self.family.as_deref().unwrap_or(&self.id);
        if family.trim().is_empty() || family == BASELINE_FAMILY || family == REALITY_FAMILY {
            return Err(Error::invalid(format!(
                "driver {} names the scenario family {family:?}, which is empty or reserved \
                 for the tree's own branches; name the class of world the driver explores",
                self.id
            )));
        }
        Ok(family.to_string())
    }
}

/// One step of an assumed propagation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropagationStep {
    pub from: String,
    pub to: String,
    pub coefficient: f64,
}

/// A hedge a branch implies: take the opposite of the branch's worst impact
/// in the instrument whose impact offsets it best.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hedge {
    pub exposed: String,
    pub hedge_with: String,
    /// Units of `hedge_with` per unit of `exposed`, negative when the hedge
    /// is the same direction as the exposure.
    pub ratio: f64,
}

/// Whether a branch is still being explored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum BranchStatus {
    Live,
    Pruned { at: Timestamp, reason: String },
}

/// One synthetic future. See the module doc for what it may never be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub parent: Option<String>,
    /// The scenario family the branch belongs to: its driver's, the
    /// [`BASELINE_FAMILY`], or for a split branch its nearest parent's (the
    /// [`REALITY_FAMILY`] when it had none).
    pub family: String,
    pub provenance: Provenance,
    pub probability: f64,
    /// The causal assumptions that produced it: `shock_sigma`,
    /// `propagation`, and for a split branch `split_from_reality`.
    pub assumptions: BTreeMap<String, f64>,
    pub evidence: Vec<String>,
    pub propagation: Vec<PropagationStep>,
    /// Hypothesised move per instrument over the horizon, in basis points.
    pub impact_bps: BTreeMap<String, f64>,
    pub hedges: Vec<Hedge>,
    pub status: BranchStatus,
    /// Whether the branch's shock is the grid's largest and beyond three
    /// standard deviations of the fitted dynamics — a move the fitted history
    /// makes implausible, so a world it does not contain. Judged against the
    /// compact state's volatility, because the twin keeps no copy of the
    /// history to search for the largest move it held.
    pub beyond_history: bool,
}

impl Branch {
    /// This branch as the §24.9 [`qip_contracts::WorldBranch`] contract, the
    /// form a branch crosses between brains in. Its causal deltas are the
    /// propagation the branch assumed, in signed basis points of a unit
    /// effect; a coefficient beyond ±1 is refused by the contract rather
    /// than clamped here. The baseline branch, which assumes no propagation,
    /// is the parent's own future and has no delta to state, so it is
    /// refused — the contract's own rule that a branch identical to its
    /// parent is not a different future.
    pub fn to_world_branch(&self) -> Result<qip_contracts::WorldBranch> {
        use qip_contracts::world_model::{BranchParent, CausalEdge, WorldBranchFields};
        let mut causal_deltas = Vec::new();
        for step in &self.propagation {
            let strength = (step.coefficient * 10_000.0).round();
            if !strength.is_finite() || strength.abs() > f64::from(i32::MAX) {
                return Err(Error::invalid(format!(
                    "branch {} assumes a propagation coefficient of {} that no basis-point \
                     strength can state",
                    self.id, step.coefficient
                )));
            }
            causal_deltas.push(CausalEdge {
                cause: step.from.clone(),
                effect: step.to.clone(),
                // Bounded just above; the contract refuses beyond ±10,000.
                strength_bp: strength as i32,
            });
        }
        let probability_bp = (self.probability * 10_000.0).round();
        qip_contracts::Validated::new(WorldBranchFields {
            schema_version: 1,
            branch_id: self.id.clone(),
            parent: match &self.parent {
                Some(id) => BranchParent::Branch { id: id.clone() },
                None => BranchParent::Root,
            },
            assumptions: self
                .assumptions
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect(),
            probability_bp: if probability_bp.is_finite() && probability_bp >= 0.0 {
                probability_bp as u32
            } else {
                return Err(Error::invalid(format!(
                    "branch {} has probability {}, which is not a probability",
                    self.id, self.probability
                )));
            },
            causal_deltas,
            unresolved_evidence: self.evidence.clone(),
        })
    }
}

/// One live branch's implication for a portfolio: how likely the world is and
/// how far the portfolio moves in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortfolioOutcome {
    pub branch_id: String,
    /// The branch's share of the live probability mass; the weights of one
    /// distribution sum to one.
    pub weight: f64,
    /// Hypothesised portfolio move over the horizon, in basis points of net
    /// asset value.
    pub portfolio_bps: f64,
}

/// One scenario family's live branches as a distribution of its own
/// (R12-WORLD-9035).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilyPartition {
    /// The family's share of the tree's live mass. The shares of every
    /// family in one partition sum to one.
    pub mass: f64,
    /// Each live member's probability conditional on the family, by branch
    /// id. The weights of one family sum to one.
    pub weights: BTreeMap<String, f64>,
}

/// The tree's live branches as compact [`qip_contracts::WorldBranch`]
/// records, partitioned by scenario family (R12-WORLD-9035).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilyBranches {
    /// The records of each family, in branch-id order.
    pub records: BTreeMap<String, Vec<qip_contracts::WorldBranch>>,
    /// Live branches with no record, by id, with the contract's reason. The
    /// baseline and a split branch assume no propagation and so state no
    /// causal delta, which the contract refuses; they are listed here rather
    /// than dropped, so a reader can count what the records leave out.
    pub unrecorded: BTreeMap<String, String>,
}

/// How a tree grows and is pruned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeConfig {
    pub horizon: Duration,
    /// Shock sizes in standard deviations of the horizon move, each taken
    /// with both signs.
    pub shock_sigmas: Vec<f64>,
    /// Propagation strengths each driver is explored at.
    pub propagation: Vec<f64>,
    /// A live branch whose probability falls below this is pruned.
    pub prune_below: f64,
    /// Reality further than this many standard deviations from every live
    /// branch splits a new one.
    pub split_beyond_sigmas: f64,
    pub max_branches: usize,
}

impl TreeConfig {
    fn validate(&self) -> Result<()> {
        if self.horizon <= Duration::ZERO {
            return Err(Error::invalid("a future tree needs a positive horizon"));
        }
        if self.shock_sigmas.is_empty()
            || self
                .shock_sigmas
                .iter()
                .any(|s| !s.is_finite() || *s <= 0.0)
        {
            return Err(Error::invalid(
                "shock sizes are positive, finite standard deviations; signs are explored by \
                 the tree",
            ));
        }
        if self.propagation.is_empty()
            || self.propagation.iter().any(|p| !p.is_finite() || *p < 0.0)
        {
            return Err(Error::invalid(
                "propagation strengths are finite and non-negative",
            ));
        }
        let prune_ok = self.prune_below > 0.0 && self.prune_below < 1.0;
        let split_ok = self.split_beyond_sigmas.is_finite() && self.split_beyond_sigmas > 0.0;
        if !prune_ok || !split_ok || self.max_branches < 2 {
            return Err(Error::invalid(
                "a tree prunes below a probability in (0, 1), splits beyond a positive distance \
                 and holds at least two branches",
            ));
        }
        Ok(())
    }
}

/// What actually happened over the horizon, in basis points per instrument,
/// as measured — the only way reality reaches a tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObservedMoves {
    /// When the moves were measured. Must be after the tree's state.
    pub at: Timestamp,
    pub moves_bps: BTreeMap<String, f64>,
}

/// What one observation did to the tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObservationReport {
    /// Log probability the tree assigned to what happened.
    pub log_score: f64,
    pub pruned: Vec<String>,
    pub split: Option<String>,
    pub live: usize,
}

/// A tree of synthetic futures. See the module doc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FutureTree {
    pub state_digest: String,
    pub grown_at: Timestamp,
    pub config: TreeConfig,
    state: TwinState,
    pub branches: BTreeMap<String, Branch>,
    /// Summed log score over every observation.
    pub log_score: f64,
    pub observations: u64,
}

impl FutureTree {
    /// Grow a tree from `state` over `drivers`. Refuses a driver set with no
    /// instrument the state knows, and a tree larger than the bound.
    pub fn grow(state: &TwinState, drivers: &[WorldDriver], config: TreeConfig) -> Result<Self> {
        config.validate()?;
        let horizon_secs = config.horizon.as_secs_f64();
        let mut branches = BTreeMap::new();
        let mut weights = Vec::new();
        let max_sigma = config.shock_sigmas.iter().copied().fold(0.0_f64, f64::max);
        // The baseline branch: drift only, the world continuing.
        let mut baseline = BTreeMap::new();
        for (instrument, dynamics) in &state.instruments {
            baseline.insert(
                instrument.clone(),
                dynamics.drift_bps_per_sec * horizon_secs,
            );
        }
        let mut pending = vec![(
            "baseline".to_string(),
            BASELINE_FAMILY.to_string(),
            BTreeMap::from([
                ("shock_sigma".to_string(), 0.0),
                ("propagation".to_string(), 0.0),
            ]),
            Vec::new(),
            Vec::new(),
            baseline,
            1.0,
            false,
        )];
        for driver in drivers {
            let family = driver.family()?;
            let reached: Vec<(&String, &f64, &InstrumentDynamics)> = driver
                .sensitivities
                .iter()
                .filter_map(|(i, s)| state.instruments.get(i).map(|d| (i, s, d)))
                .collect();
            if reached.is_empty() {
                continue;
            }
            for sigma in &config.shock_sigmas {
                for sign in [1.0, -1.0] {
                    for strength in &config.propagation {
                        let shock = sigma * sign;
                        let mut impact = BTreeMap::new();
                        let mut steps = Vec::new();
                        for (instrument, dynamics) in &state.instruments {
                            let own = dynamics.drift_bps_per_sec * horizon_secs;
                            let scale = dynamics.vol_bps_per_sqrt_sec * horizon_secs.sqrt();
                            let direct = driver.sensitivities.get(instrument).copied();
                            let moved = match direct {
                                Some(sensitivity) => {
                                    steps.push(PropagationStep {
                                        from: driver.id.clone(),
                                        to: instrument.clone(),
                                        coefficient: sensitivity,
                                    });
                                    shock * sensitivity * scale
                                }
                                // Second order: the common move carried to
                                // an instrument the driver does not reach,
                                // through its beta, at the assumed strength.
                                None => {
                                    let mean_direct: f64 = reached
                                        .iter()
                                        .map(|(_, s, d)| {
                                            shock
                                                * **s
                                                * d.vol_bps_per_sqrt_sec
                                                * horizon_secs.sqrt()
                                        })
                                        .sum::<f64>()
                                        / reached.len() as f64;
                                    if *strength > 0.0 {
                                        steps.push(PropagationStep {
                                            from: driver.id.clone(),
                                            to: instrument.clone(),
                                            coefficient: strength * dynamics.beta,
                                        });
                                    }
                                    mean_direct * strength * dynamics.beta
                                }
                            };
                            impact.insert(instrument.clone(), own + moved);
                        }
                        // Prior: a standard-normal weight on the shock size.
                        let prior = (-0.5 * shock * shock).exp();
                        let id = format!("{}:s{shock:+.2}:p{strength:.2}", driver.id);
                        pending.push((
                            id,
                            family.clone(),
                            BTreeMap::from([
                                ("shock_sigma".to_string(), shock),
                                ("propagation".to_string(), *strength),
                            ]),
                            driver.evidence.clone(),
                            steps,
                            impact,
                            prior,
                            *sigma >= max_sigma && *sigma > 3.0,
                        ));
                    }
                }
            }
        }
        if pending.len() > config.max_branches {
            return Err(Error::invalid(format!(
                "the drivers and grid grow {} branch(es), more than the {} this tree may hold; \
                 narrow the grid rather than have branches dropped unexplored",
                pending.len(),
                config.max_branches
            )));
        }
        for (id, family, assumptions, evidence, propagation, impact, prior, beyond) in pending {
            let hedges = hedge_for(&impact);
            weights.push(prior);
            branches.insert(
                id.clone(),
                Branch {
                    id,
                    parent: None,
                    family,
                    provenance: Provenance::Synthetic,
                    probability: prior,
                    assumptions,
                    evidence,
                    propagation,
                    impact_bps: impact,
                    hedges,
                    status: BranchStatus::Live,
                    beyond_history: beyond,
                },
            );
        }
        let total: f64 = weights.iter().sum();
        for branch in branches.values_mut() {
            branch.probability /= total;
        }
        Ok(Self {
            state_digest: state.digest()?,
            grown_at: state.as_of,
            config,
            state: state.clone(),
            branches,
            log_score: 0.0,
            observations: 0,
        })
    }

    /// The compact state the tree was grown from.
    pub const fn state(&self) -> &TwinState {
        &self.state
    }

    /// Live branches, in id order.
    pub fn live(&self) -> impl Iterator<Item = &Branch> {
        self.branches
            .values()
            .filter(|b| b.status == BranchStatus::Live)
    }

    /// The portfolio outcome each live branch implies, as a probability
    /// distribution over portfolio moves (R12-TICK-10121).
    ///
    /// `positions` is the signed weight of each instrument in the portfolio
    /// (a fraction of net asset value, negative for a short), so a branch's
    /// outcome is `sum(weight * impact_bps)` in basis points of the whole
    /// portfolio. Live branch probabilities are renormalised to sum to one,
    /// because pruning removes branches without redistributing their mass
    /// and an outcome distribution whose weights summed to less than one
    /// would understate every tail. An instrument a branch has no impact for
    /// contributes nothing, since the branch asserts no move for it.
    ///
    /// The weights and impacts are statistics, so they stay `f64`; no money
    /// amount crosses here. Refuses an empty or non-finite position set,
    /// positions that touch no instrument the tree models (an outcome of
    /// zero everywhere would read as a hedged book), and a tree with no live
    /// mass, rather than returning an empty distribution.
    pub fn portfolio_outcomes(
        &self,
        positions: &BTreeMap<String, f64>,
    ) -> Result<Vec<PortfolioOutcome>> {
        if positions.is_empty() {
            return Err(Error::invalid(
                "a portfolio outcome needs at least one position; an empty book has no outcome",
            ));
        }
        for (instrument, weight) in positions {
            if !weight.is_finite() {
                return Err(Error::invalid(format!(
                    "position {instrument} has weight {weight}, which is not a finite number"
                )));
            }
        }
        if !positions
            .keys()
            .any(|instrument| self.state.instruments.contains_key(instrument))
        {
            return Err(Error::invalid(
                "no position names an instrument the tree models, so every outcome would read \
                 as zero; position the instruments the twin was fitted on",
            ));
        }
        let live: Vec<&Branch> = self.live().collect();
        let mass: f64 = live.iter().map(|b| b.probability).sum();
        if live.is_empty() || !mass.is_finite() || mass <= 0.0 {
            return Err(Error::invalid(
                "the tree has no live branch with probability, so there is no distribution \
                 to state",
            ));
        }
        Ok(live
            .into_iter()
            .map(|branch| PortfolioOutcome {
                branch_id: branch.id.clone(),
                weight: branch.probability / mass,
                portfolio_bps: positions
                    .iter()
                    .map(|(instrument, weight)| {
                        weight * branch.impact_bps.get(instrument).copied().unwrap_or(0.0)
                    })
                    .sum(),
            })
            .collect())
    }

    /// The live branches partitioned by scenario family, each family a
    /// distribution of its own and the families together a distribution over
    /// scenarios (R12-WORLD-9035). Pruning removes a branch from its family
    /// without moving its mass to another, so a family that reality has made
    /// implausible loses share rather than lending its members' weight to a
    /// family it never explored. Refuses a tree with no live mass, rather
    /// than returning families whose weights sum to nothing.
    pub fn families(&self) -> Result<BTreeMap<String, FamilyPartition>> {
        let mass: f64 = self.live().map(|b| b.probability).sum();
        if !mass.is_finite() || mass <= 0.0 {
            return Err(Error::invalid(
                "the tree has no live branch with probability, so it has no family to state",
            ));
        }
        let mut members: BTreeMap<&str, Vec<&Branch>> = BTreeMap::new();
        for branch in self.live() {
            members.entry(&branch.family).or_default().push(branch);
        }
        let mut families = BTreeMap::new();
        for (family, branches) in members {
            let within: f64 = branches.iter().map(|b| b.probability).sum();
            // A family whose every member sits at zero has no conditional
            // distribution; it is left out rather than given equal weights
            // nobody computed.
            if within <= 0.0 {
                continue;
            }
            families.insert(
                family.to_string(),
                FamilyPartition {
                    mass: within / mass,
                    weights: branches
                        .iter()
                        .map(|b| (b.id.clone(), b.probability / within))
                        .collect(),
                },
            );
        }
        Ok(families)
    }

    /// The live branches as compact [`qip_contracts::WorldBranch`] records,
    /// partitioned by scenario family (R12-WORLD-9035). A branch the
    /// contract refuses is listed with the refusal, never dropped.
    pub fn world_branches_by_family(&self) -> FamilyBranches {
        let mut records: BTreeMap<String, Vec<qip_contracts::WorldBranch>> = BTreeMap::new();
        let mut unrecorded = BTreeMap::new();
        for branch in self.live() {
            match branch.to_world_branch() {
                Ok(record) => records
                    .entry(branch.family.clone())
                    .or_default()
                    .push(record),
                Err(error) => {
                    unrecorded.insert(branch.id.clone(), error.message().to_string());
                }
            }
        }
        FamilyBranches {
            records,
            unrecorded,
        }
    }

    /// Standardised distance of `moves` from `branch`, over the instruments
    /// both name.
    fn distance(&self, branch: &Branch, moves: &BTreeMap<String, f64>) -> Option<f64> {
        let horizon_secs = self.config.horizon.as_secs_f64();
        let mut sum = 0.0;
        let mut n = 0usize;
        for (instrument, observed) in moves {
            let (Some(expected), Some(dynamics)) = (
                branch.impact_bps.get(instrument),
                self.state.instruments.get(instrument),
            ) else {
                continue;
            };
            let scale = (dynamics.vol_bps_per_sqrt_sec * horizon_secs.sqrt()).max(1e-9);
            sum += ((observed - expected) / scale).powi(2);
            n += 1;
        }
        (n > 0).then(|| (sum / n as f64).sqrt())
    }

    /// Score, reweight, prune and split on what happened. Refuses an
    /// observation at or before the tree's state — reality from before the
    /// fork is history the twin was fitted on, not a test of its futures —
    /// and one naming no instrument the tree models.
    pub fn observe(&mut self, observed: &ObservedMoves) -> Result<ObservationReport> {
        if observed.at <= self.grown_at {
            return Err(Error::invalid(format!(
                "an observation at {} does not follow the tree's state at {}; it cannot score \
                 a future",
                observed.at, self.grown_at
            )));
        }
        let mut likelihoods = BTreeMap::new();
        let mut nearest = f64::INFINITY;
        let mut best: Option<String> = None;
        for branch in self.live() {
            let Some(d) = self.distance(branch, &observed.moves_bps) else {
                continue;
            };
            if d < nearest {
                nearest = d;
                best = Some(branch.id.clone());
            }
            likelihoods.insert(branch.id.clone(), (-0.5 * d * d).exp());
        }
        if likelihoods.is_empty() {
            return Err(Error::invalid(
                "the observation names no instrument the tree models; it scores nothing",
            ));
        }
        let evidence: f64 = likelihoods
            .iter()
            .map(|(id, l)| self.branches.get(id).map_or(0.0, |b| b.probability) * l)
            .sum();
        // Log score of the tree's mixture at what happened; floored so one
        // impossible observation is a large finite penalty, not -inf.
        let log_score = evidence.max(1e-300).ln();
        self.log_score += log_score;
        self.observations += 1;
        let mut pruned = Vec::new();
        for (id, likelihood) in &likelihoods {
            if let Some(branch) = self.branches.get_mut(id) {
                branch.probability = if evidence > 0.0 {
                    branch.probability * likelihood / evidence
                } else {
                    0.0
                };
            }
        }
        for branch in self.branches.values_mut() {
            if branch.status == BranchStatus::Live && branch.probability < self.config.prune_below {
                branch.status = BranchStatus::Pruned {
                    at: observed.at,
                    reason: format!(
                        "probability {:.4} after reality at {}",
                        branch.probability, observed.at
                    ),
                };
                pruned.push(branch.id.clone());
            }
        }
        let mut split = None;
        if nearest > self.config.split_beyond_sigmas
            && self.branches.len() < self.config.max_branches
        {
            let parent = best.clone();
            let family = parent
                .as_ref()
                .and_then(|id| self.branches.get(id))
                .map_or_else(|| REALITY_FAMILY.to_string(), |b| b.family.clone());
            let id = format!("split:{}:{}", observed.at.as_nanos(), self.observations);
            let mut impact = BTreeMap::new();
            for instrument in self.state.instruments.keys() {
                if let Some(m) = observed.moves_bps.get(instrument) {
                    impact.insert(instrument.clone(), *m);
                }
            }
            let live = self.live().count().max(1) as f64;
            let probability = 1.0 / (live + 1.0);
            for branch in self.branches.values_mut() {
                if branch.status == BranchStatus::Live {
                    branch.probability *= 1.0 - probability;
                }
            }
            let hedges = hedge_for(&impact);
            self.branches.insert(
                id.clone(),
                Branch {
                    id: id.clone(),
                    parent,
                    family,
                    provenance: Provenance::Synthetic,
                    probability,
                    assumptions: BTreeMap::from([("split_from_reality".to_string(), nearest)]),
                    evidence: vec![format!("reality at {} outside every branch", observed.at)],
                    propagation: Vec::new(),
                    impact_bps: impact,
                    hedges,
                    status: BranchStatus::Live,
                    beyond_history: false,
                },
            );
            split = Some(id);
        }
        // Renormalise what is live so the tree stays a distribution.
        let total: f64 = self.live().map(|b| b.probability).sum();
        if total > 0.0 {
            for branch in self.branches.values_mut() {
                if branch.status == BranchStatus::Live {
                    branch.probability /= total;
                }
            }
        }
        Ok(ObservationReport {
            log_score,
            pruned,
            split,
            live: self.live().count(),
        })
    }
}

/// The hedge for the worst-hit instrument: the instrument whose impact best
/// offsets it, at the ratio that flattens the move.
fn hedge_for(impact: &BTreeMap<String, f64>) -> Vec<Hedge> {
    let Some((worst, worst_move)) = impact
        .iter()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .map(|(k, v)| (k.clone(), *v))
    else {
        return Vec::new();
    };
    if worst_move >= 0.0 {
        return Vec::new();
    }
    impact
        .iter()
        .filter(|(k, v)| **k != worst && v.abs() > 1e-9)
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map(|(k, v)| {
            vec![Hedge {
                exposed: worst.clone(),
                hedge_with: k.clone(),
                ratio: -worst_move / v,
            }]
        })
        .unwrap_or_default()
}
