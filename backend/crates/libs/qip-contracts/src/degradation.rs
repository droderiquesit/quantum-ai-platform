//! What the platform does when a cognitive capability is unavailable or stale.
//!
//! The rule this module states is that **losing a capability narrows the
//! platform; it never halts it**. That distinction is the whole point. A
//! system that halts on a stale input is a system that stops trading every
//! time a warm-path job is late, and the operational pressure to "just ignore
//! the staleness" then becomes irresistible — which is how a safety property
//! gets deleted by the people it protects.
//!
//! **Which rows fire, and where.** The policy-fed rows are consumed: the
//! edge cell sizes every pass by [`DegradationState::sizing_multiplier`] over
//! the narrowing `PolicyPayload::narrowing` derives from the signed payload of
//! blueprint §41.5. At the centre, [`SelfModelFreshness`] derives the
//! self-model row from the learning engine's record and
//! [`DegradationState::central_sizing_multiplier`] compounds it; the kernel's
//! `Platform::central_degradation` calls both at sizing time. The centre's
//! own causal-graph and belief-state rows are derived the same way —
//! [`CausalGraphFreshness`] and [`BeliefFreshness`] from the instant each
//! last absorbed evidence — and are specifications until that same seam
//! observes them: a row the kernel does not observe reads fresh there by
//! construction, which is the overclaim this module's own reasoning rejects
//! elsewhere. The reason the valuation row is still omitted below is that a
//! control which cannot fire reads as protection and is not.
//!
//! This is *capability*-level degradation and it composes with, rather than
//! replaces, the mechanism-level rules already in the tree: a stale book
//! supplying nothing (`qip-edge`'s seam) and venue health (`qip-routing`) are
//! about one book and one venue, and answer a different question from "the
//! causal graph has not been re-estimated for an hour, so how large may we
//! size?".
//!
//! Two design decisions carry the safety argument:
//!
//! * **Absence is the worst case, not the best.** [`DegradationState`] reports
//!   [`Freshness::Unavailable`] for a capability nobody has said anything
//!   about. A capability whose reporter has itself died would otherwise read
//!   as healthy, which is precisely the failure mode that makes a monitoring
//!   gap indistinguishable from good news.
//! * **Degradations compound rather than compete.** Two independent reasons to
//!   distrust a size multiply, so adding a degradation can only ever make the
//!   multiplier smaller. Taking the most conservative single rule instead
//!   would let a second, independent loss of confidence cost nothing.
//!
//! Only the capabilities this repository actually has are represented.
//! Blueprint §6.2 also names a valuation engine; none exists here, and
//! inventing an enum variant for a capability that can never be unavailable
//! would be a control that cannot fire — a mistake this repository has
//! already made nine times and does not need a tenth of. The self-model row
//! was omitted on the same argument until `qip-learning-engine`'s `SelfModel`
//! existed; it does now, and it can be thin or empty, so the row can fire.

use qip_core::{Decimal, Duration, Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A cognitive capability whose loss changes what the platform will do.
///
/// Ordered, and iterated through [`BTreeMap`], because the narrowing a state
/// reports reaches an operator's screen and a replay that reorders is not a
/// replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Observation of the world. Blueprint §6.2 row 1.
    Ingestion,
    /// The causal graph over drivers. Row 2.
    CausalGraph,
    /// Episodic memory and analogical retrieval. Row 3.
    EpisodicMemory,
    /// The belief state, whose confidence drives size. Row 4.
    BeliefState,
    /// Counterfactual scoring of paths not taken. Row 5.
    CounterfactualScoring,
    /// The platform's estimate of its own components' reliability. Row 6.
    ///
    /// Held by the centre alone: the LEARN stage feeds it and the REASON
    /// stage scales evidence by it. No policy item ships it to a cell, so a
    /// cell's table reads it as unavailable by default — truthfully, since
    /// the cell has no such model — and the edge's sizing deliberately does
    /// not compound it; see [`DegradationState::central_sizing_multiplier`].
    SelfModel,
}

impl Capability {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ingestion => "ingestion",
            Self::CausalGraph => "causal_graph",
            Self::EpisodicMemory => "episodic_memory",
            Self::BeliefState => "belief_state",
            Self::CounterfactualScoring => "counterfactual_scoring",
            Self::SelfModel => "self_model",
        }
    }

    pub const fn all() -> [Self; 6] {
        [
            Self::Ingestion,
            Self::CausalGraph,
            Self::EpisodicMemory,
            Self::BeliefState,
            Self::CounterfactualScoring,
            Self::SelfModel,
        ]
    }

    /// Whether losing this capability may change a trading decision at all.
    ///
    /// Blueprint §6.2 is explicit that counterfactual scoring has "no trading
    /// impact whatsoever" — it is entirely a warm-path function. Stating that
    /// as a method rather than as a comment is what stops a later change from
    /// quietly giving the learning path a veto over the trading path.
    pub const fn affects_trading(&self) -> bool {
        !matches!(self, Self::CounterfactualScoring)
    }
}

/// How current a capability's state is.
///
/// Deliberately three-valued rather than a boolean. "Stale" and "unavailable"
/// narrow by different amounts in §6.2 — a stale belief falls back to a fixed
/// multiplier while an absent one is the same thing only because we choose to
/// treat it that way — and collapsing them would lose the distinction the
/// table draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    /// Current, within its TTL.
    Fresh,
    /// Past its TTL but still readable.
    Stale,
    /// Not readable at all.
    Unavailable,
}

impl Freshness {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }

    /// Parse a freshness token, refusing anything it does not recognise.
    ///
    /// Refuses rather than defaulting, on the standing rule that a value
    /// silently corrected is a caller bug that survives. The refusal names the
    /// permitted tokens because an error that does not say what to do instead
    /// is a dead end.
    ///
    /// The match is on the whole string, not a prefix or a substring: `"stale"`
    /// is a substring of a great many things, and a policy file with a typo
    /// that happened to contain it would otherwise be read as a deliberate
    /// degradation.
    pub fn parse(token: &str) -> Result<Self> {
        match token {
            "fresh" => Ok(Self::Fresh),
            "stale" => Ok(Self::Stale),
            "unavailable" => Ok(Self::Unavailable),
            other => Err(Error::invalid(format!(
                "unknown freshness {other:?}; expected one of fresh, stale, unavailable"
            ))),
        }
    }

    /// Whether the capability is usable at full strength.
    pub const fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh)
    }
}

/// The classes of strategy §6.2 treats differently when a capability is lost.
///
/// The table's rows are not "everything pauses" — they are careful about which
/// strategies keep running, and that care is the useful part. A price-only
/// strategy does not consult the world model, so an ingestion stall is not its
/// problem, and pausing it would be an outage the platform inflicted on itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyClass {
    /// Depends on prices and books alone.
    PriceOnly,
    /// Depends on world events — filings, releases, news.
    EventDriven,
    /// Depends on resolution of a real-world outcome.
    PredictionMarket,
    /// Depends on recognising a situation it has seen before.
    SituationalRecognition,
}

impl StrategyClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::PriceOnly => "price_only",
            Self::EventDriven => "event_driven",
            Self::PredictionMarket => "prediction_market",
            Self::SituationalRecognition => "situational_recognition",
        }
    }

    pub const fn all() -> [Self; 4] {
        [
            Self::PriceOnly,
            Self::EventDriven,
            Self::PredictionMarket,
            Self::SituationalRecognition,
        ]
    }
}

/// How capital is spread across families.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationMode {
    /// Conditioned on the detected regime — the full-capability mode.
    RegimeConditional,
    /// The fallback when the causal graph can no longer be reasoned about.
    Unconditional,
}

/// The multiplier applied to size when the causal graph cannot be trusted.
///
/// §6.2: "Sizing becomes more conservative because relationships can no longer
/// be reasoned about." The number is a policy choice and this is the one place
/// it is written down; what the type system holds is that it is below one and
/// that it compounds.
const CAUSAL_STALE_MULTIPLIER: (i128, u32) = (75, 2);

/// The fixed conservative multiplier §6.2 row 4 falls back to when the belief
/// state is stale beyond its TTL.
///
/// Fixed on purpose: the point of the fallback is that it does not depend on a
/// confidence estimate, because the confidence estimate is the thing that has
/// gone stale.
const BELIEF_STALE_MULTIPLIER: (i128, u32) = (50, 2);

/// The multiplier §6.2 row 6 applies when the self-model is thin or old.
///
/// A thin model is still a model: some components are weighted on a measured
/// record and the rest are left at full weight because nothing has measured
/// them. The narrowing is for the second group, whose weight is a default
/// rather than an estimate.
const SELF_MODEL_STALE_MULTIPLIER: (i128, u32) = (75, 2);

/// The multiplier §6.2 row 6 applies when the self-model has never absorbed
/// an outcome.
///
/// Smaller than the stale case on purpose. A model that has absorbed nothing
/// weights *every* component at full weight on no evidence at all — the
/// state the self-model exists to end — and a sizing that treated it like a
/// merely thin model would size a platform that has never checked itself as
/// if it had.
const SELF_MODEL_UNAVAILABLE_MULTIPLIER: (i128, u32) = (50, 2);

/// How old the self-model's newest graded outcome may be before the row is
/// stale.
///
/// A policy choice, and this is the one place it is written down. The LEARN
/// stage grades a thesis when it resolves, and theses resolve over days; a
/// week with no graded outcome on any component means either that LEARN has
/// stopped running or that nothing resolved, and either way the record
/// describes a platform a week older than the one now sizing.
pub const SELF_MODEL_HORIZON: Duration = Duration::from_days(7);

/// How old the causal graph's newest absorbed claim may be before §6.2 row 2
/// reads stale.
///
/// A policy choice, and this is the one place it is written down. Causal
/// claims are recorded from filings and research notes, and filings arrive
/// on a quarterly cycle; a graph that has absorbed no claim in a full quarter
/// has sat through at least one filing season without being re-estimated,
/// and the relationships it propagates a shock along are the ones a quarter
/// ago's evidence supported.
pub const CAUSAL_GRAPH_HORIZON: Duration = Duration::from_days(90);

/// How old the belief state's newest formed hypothesis may be before §6.2
/// row 4 reads stale.
///
/// A policy choice, and this is the one place it is written down. The REASON
/// stage forms a belief from evidence as of a cycle, and the confidence it
/// computes is what drives size; a session with nothing formed means the
/// centre is sizing against a confidence that read the market a session ago,
/// which is exactly the case row 4's fixed fallback exists for.
pub const BELIEF_HORIZON: Duration = Duration::from_days(1);

/// Why the self-model row of §6.2 reads as it does.
///
/// An enum rather than a bare [`Freshness`] because the table's consumer has
/// to say what to do about a narrowing, and "stale" alone does not say
/// whether a component needs more outcomes or LEARN needs to run. Every arm
/// carries the fact that produced it, so the reason is reported from the
/// same computation that narrowed and cannot drift from it.
///
/// Derived by [`SelfModelFreshness::assess`] from what the learning engine's
/// `SelfModel` reports about each component — its sample count and the
/// instant its newest outcome was graded — so that this crate, which the
/// learning engine may not depend on in reverse, needs nothing of the model
/// but those facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SelfModelFreshness {
    /// Every component the platform charges has at least the minimum sample
    /// and the newest outcome across them is within the horizon.
    Fresh {
        components: usize,
        newest: Timestamp,
    },
    /// The model has never absorbed an outcome. Reads as
    /// [`Freshness::Unavailable`]: there is no record at all, and the
    /// distinction from a thin one is the whole reason the row exists.
    NeverAbsorbed,
    /// A charged component has fewer outcomes than the minimum, so its
    /// estimate is withheld and it is weighted on no evidence. The first
    /// such component in key order is named, so two replays name the same
    /// one.
    UnderSampled {
        component: String,
        samples: usize,
        minimum: usize,
    },
    /// Every component has its sample, but the newest outcome across them is
    /// older than the horizon.
    BeyondHorizon {
        newest: Timestamp,
        age: Duration,
        horizon: Duration,
    },
}

impl SelfModelFreshness {
    /// Derive the row's freshness from the model's per-component record.
    ///
    /// `records` yields, per charged component, its key, its sample count
    /// and when its newest outcome was graded — the shape the learning
    /// engine's `SelfModel::sample_facts` produces. `minimum_sample` is the
    /// engine's own bar below which it withholds an estimate; passing it in
    /// rather than restating it here is what keeps the two from disagreeing.
    ///
    /// Refuses rather than guesses on the two inputs that would widen: a
    /// minimum of zero calls an empty record measured, and a horizon of zero
    /// or less is a configuration nothing can be within. A record claiming
    /// outcomes but no newest instant is refused too — it is the reporter's
    /// bug, and reading it as either fresh or stale would hide it.
    ///
    /// A newest instant *after* `now` is not treated as stale: its age is
    /// negative and negative is within the horizon. That is deliberate — a
    /// replay that grades ahead of the clock it sizes against has a record,
    /// and the table's job is to narrow on the absence of one.
    pub fn assess<I>(
        records: I,
        minimum_sample: usize,
        horizon: Duration,
        now: Timestamp,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = (String, usize, Option<Timestamp>)>,
    {
        if minimum_sample == 0 {
            return Err(Error::invalid(
                "a self-model minimum sample of 0 would call an empty record measured; pass \
                 the learning engine's minimum",
            ));
        }
        if horizon <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "a self-model horizon of {horizon:?} is one nothing can be within; pass a \
                 positive horizon"
            )));
        }
        let mut components = 0usize;
        let mut newest: Option<Timestamp> = None;
        for (component, samples, last_graded) in records {
            components += 1;
            if samples < minimum_sample {
                return Ok(Self::UnderSampled {
                    component,
                    samples,
                    minimum: minimum_sample,
                });
            }
            let Some(last_graded) = last_graded else {
                return Err(Error::invalid(format!(
                    "self-model component {component} reports {samples} outcome(s) but no \
                     newest instant; the record is inconsistent"
                )));
            };
            newest = Some(match newest {
                Some(held) if held >= last_graded => held,
                _ => last_graded,
            });
        }
        let Some(newest) = newest else {
            return Ok(Self::NeverAbsorbed);
        };
        let age = now.since(newest);
        if age > horizon {
            return Ok(Self::BeyondHorizon {
                newest,
                age,
                horizon,
            });
        }
        Ok(Self::Fresh { components, newest })
    }

    /// The table's three-valued reading of this outcome.
    ///
    /// Thin and old both read as [`Freshness::Stale`] — a record exists and
    /// is worth less — while never-absorbed is [`Freshness::Unavailable`],
    /// and the two narrow by different multipliers.
    pub const fn freshness(&self) -> Freshness {
        match self {
            Self::Fresh { .. } => Freshness::Fresh,
            Self::UnderSampled { .. } | Self::BeyondHorizon { .. } => Freshness::Stale,
            Self::NeverAbsorbed => Freshness::Unavailable,
        }
    }
}

impl fmt::Display for SelfModelFreshness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fresh { components, newest } => write!(
                f,
                "self-model fresh: {components} component(s) at or above the minimum sample, \
                 newest graded at {newest:?}"
            ),
            Self::NeverAbsorbed => write!(
                f,
                "self-model unavailable: it has never absorbed a graded outcome"
            ),
            Self::UnderSampled {
                component,
                samples,
                minimum,
            } => write!(
                f,
                "self-model stale: {component} has {samples} graded outcome(s), below the \
                 {minimum} an estimate needs"
            ),
            Self::BeyondHorizon {
                newest,
                age,
                horizon,
            } => write!(
                f,
                "self-model stale: newest outcome graded at {newest:?} is {age:?} old, past \
                 the {horizon:?} horizon"
            ),
        }
    }
}

/// The reading shared by the two rows that are judged on one instant alone.
///
/// The causal graph and the belief state each carry a single fact — when
/// they last absorbed evidence — so their rows are one arithmetic, written
/// once here and named twice below so that a reason reaching a screen says
/// which row it is about. Not public: the named types are the contract.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Recency {
    Fresh {
        last_updated: Timestamp,
    },
    NeverUpdated,
    BeyondHorizon {
        last_updated: Timestamp,
        age: Duration,
        horizon: Duration,
    },
}

/// Derive a single-instant row's recency, refusing what would widen it.
///
/// Unlike [`SelfModelFreshness::assess`], a `last_updated` after `now` is
/// refused rather than read as within the horizon. The self-model's newest
/// instant is a grade from a LEARN replay that may legitimately run ahead
/// of the sizing clock; these two instants are recorded by this process at
/// the seam where it absorbed evidence, and one ahead of the clock it is
/// now sizing against means a clock or a replay is wrong. Reading it as
/// fresh would size on the strength of a bug.
fn assess_recency(
    row: &str,
    last_updated: Option<Timestamp>,
    horizon: Duration,
    now: Timestamp,
) -> Result<Recency> {
    if horizon <= Duration::ZERO {
        return Err(Error::invalid(format!(
            "a {row} horizon of {horizon:?} is one nothing can be within; pass a positive \
             horizon"
        )));
    }
    let Some(last_updated) = last_updated else {
        return Ok(Recency::NeverUpdated);
    };
    if last_updated > now {
        return Err(Error::invalid(format!(
            "the {row} reports its last update at {last_updated:?}, after the {now:?} it is \
             being sized against; a record from the future is a clock bug, not a fresh row — \
             fix the clock rather than the reading"
        )));
    }
    let age = now.since(last_updated);
    if age > horizon {
        return Ok(Recency::BeyondHorizon {
            last_updated,
            age,
            horizon,
        });
    }
    Ok(Recency::Fresh { last_updated })
}

/// Why the causal-graph row of §6.2 reads as it does.
///
/// Derived by [`CausalGraphFreshness::assess`] from the one fact the world
/// model's `CausalGraph` records at the seam where it absorbs a claim — the
/// instant of its newest recorded edge — so this crate needs nothing of the
/// graph but that instant. Every arm carries the fact that produced it, so
/// the reason is reported from the same computation that narrowed and cannot
/// drift from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CausalGraphFreshness {
    /// The newest claim was absorbed within the horizon.
    Fresh { last_updated: Timestamp },
    /// The graph has never absorbed a claim. Reads as
    /// [`Freshness::Unavailable`]: there is no relationship to reason about,
    /// as distinct from one that is merely old.
    NeverUpdated,
    /// The newest claim is older than the horizon.
    BeyondHorizon {
        last_updated: Timestamp,
        age: Duration,
        horizon: Duration,
    },
}

impl CausalGraphFreshness {
    /// Derive the row from the graph's `last_updated` fact.
    ///
    /// Refuses a non-positive horizon and a `last_updated` after `now`; see
    /// [`assess_recency`] for why the second is a refusal here and not in the
    /// self-model row.
    pub fn assess(
        last_updated: Option<Timestamp>,
        horizon: Duration,
        now: Timestamp,
    ) -> Result<Self> {
        Ok(
            match assess_recency("causal graph", last_updated, horizon, now)? {
                Recency::Fresh { last_updated } => Self::Fresh { last_updated },
                Recency::NeverUpdated => Self::NeverUpdated,
                Recency::BeyondHorizon {
                    last_updated,
                    age,
                    horizon,
                } => Self::BeyondHorizon {
                    last_updated,
                    age,
                    horizon,
                },
            },
        )
    }

    /// The table's three-valued reading of this outcome.
    pub const fn freshness(&self) -> Freshness {
        match self {
            Self::Fresh { .. } => Freshness::Fresh,
            Self::BeyondHorizon { .. } => Freshness::Stale,
            Self::NeverUpdated => Freshness::Unavailable,
        }
    }
}

impl fmt::Display for CausalGraphFreshness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fresh { last_updated } => write!(
                f,
                "causal graph fresh: newest claim absorbed at {last_updated:?}"
            ),
            Self::NeverUpdated => {
                write!(f, "causal graph unavailable: it has never absorbed a claim")
            }
            Self::BeyondHorizon {
                last_updated,
                age,
                horizon,
            } => write!(
                f,
                "causal graph stale: newest claim absorbed at {last_updated:?} is {age:?} old, \
                 past the {horizon:?} horizon"
            ),
        }
    }
}

/// Why the belief-state row of §6.2 reads as it does.
///
/// Derived by [`BeliefFreshness::assess`] from the one fact the reasoning
/// engine's `BeliefState` records at the seam where evidence becomes a belief
/// — the instant its newest hypothesis was formed — so this crate needs
/// nothing of the engine but that instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BeliefFreshness {
    /// The newest belief was formed within the horizon.
    Fresh { last_updated: Timestamp },
    /// No belief has ever been formed. Reads as [`Freshness::Unavailable`]:
    /// there is no confidence to drive size, as distinct from one that is
    /// merely old.
    NeverUpdated,
    /// The newest belief is older than the horizon.
    BeyondHorizon {
        last_updated: Timestamp,
        age: Duration,
        horizon: Duration,
    },
}

impl BeliefFreshness {
    /// Derive the row from the belief state's `last_updated` fact.
    ///
    /// Refuses a non-positive horizon and a `last_updated` after `now`; see
    /// [`assess_recency`].
    pub fn assess(
        last_updated: Option<Timestamp>,
        horizon: Duration,
        now: Timestamp,
    ) -> Result<Self> {
        Ok(
            match assess_recency("belief state", last_updated, horizon, now)? {
                Recency::Fresh { last_updated } => Self::Fresh { last_updated },
                Recency::NeverUpdated => Self::NeverUpdated,
                Recency::BeyondHorizon {
                    last_updated,
                    age,
                    horizon,
                } => Self::BeyondHorizon {
                    last_updated,
                    age,
                    horizon,
                },
            },
        )
    }

    /// The table's three-valued reading of this outcome.
    pub const fn freshness(&self) -> Freshness {
        match self {
            Self::Fresh { .. } => Freshness::Fresh,
            Self::BeyondHorizon { .. } => Freshness::Stale,
            Self::NeverUpdated => Freshness::Unavailable,
        }
    }
}

impl fmt::Display for BeliefFreshness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fresh { last_updated } => write!(
                f,
                "belief state fresh: newest belief formed at {last_updated:?}"
            ),
            Self::NeverUpdated => write!(
                f,
                "belief state unavailable: no belief has ever been formed"
            ),
            Self::BeyondHorizon {
                last_updated,
                age,
                horizon,
            } => write!(
                f,
                "belief state stale: newest belief formed at {last_updated:?} is {age:?} old, \
                 past the {horizon:?} horizon"
            ),
        }
    }
}

/// What the platform currently believes about each capability, and what that
/// implies.
///
/// Construct with [`DegradationState::fully_available`] and record what is
/// known; anything not recorded reads as [`Freshness::Unavailable`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradationState {
    freshness: BTreeMap<Capability, Freshness>,
}

impl DegradationState {
    /// Every capability fresh. The state a healthy platform reports.
    pub fn fully_available() -> Self {
        let mut freshness = BTreeMap::new();
        for capability in Capability::all() {
            freshness.insert(capability, Freshness::Fresh);
        }
        Self { freshness }
    }

    /// Nothing known about anything, which fails closed to fully degraded.
    pub fn nothing_known() -> Self {
        Self {
            freshness: BTreeMap::new(),
        }
    }

    /// Record what is known about one capability.
    pub fn observe(&mut self, capability: Capability, freshness: Freshness) {
        self.freshness.insert(capability, freshness);
    }

    /// What is known about one capability.
    ///
    /// **Fails closed.** A capability nobody has reported on is `Unavailable`,
    /// not `Fresh`. The alternative makes a dead reporter look identical to a
    /// healthy subsystem, and the platform would size as though it still knew
    /// something it had merely stopped being told.
    pub fn freshness(&self, capability: Capability) -> Freshness {
        self.freshness
            .get(&capability)
            .copied()
            .unwrap_or(Freshness::Unavailable)
    }

    /// Whether analogical retrieval is available.
    ///
    /// §6.2 row 3 has two clauses — "analogical retrieval unavailable" and
    /// "strategies depending on situational recognition pause" — and only the
    /// second had an accessor. A caller that wants to know whether it may
    /// *retrieve* an analogue, rather than whether it must stop, had to infer
    /// it from a strategy class it may not have.
    pub fn analogical_retrieval_available(&self) -> bool {
        self.freshness(Capability::EpisodicMemory).is_fresh()
    }

    /// Whether a strategy of this class pauses.
    ///
    /// Straight from §6.2, and the negative half matters as much as the
    /// positive: a price-only strategy pauses for nothing in this table,
    /// because nothing in this table is an input to it.
    pub fn pauses(&self, class: StrategyClass) -> bool {
        let ingestion_lost = !self.freshness(Capability::Ingestion).is_fresh();
        let episodic_lost = !self.freshness(Capability::EpisodicMemory).is_fresh();
        match class {
            // Not a default arm. Written out so that adding a class forces a
            // decision here rather than inheriting "never pauses" by accident.
            StrategyClass::PriceOnly => false,
            StrategyClass::EventDriven | StrategyClass::PredictionMarket => ingestion_lost,
            StrategyClass::SituationalRecognition => episodic_lost,
        }
    }

    /// The multiplier applied to position size under the current degradation.
    ///
    /// `Decimal` and never `f64`: this scales a position, so it is money
    /// arithmetic however statistical its inputs were.
    ///
    /// Compounding rather than competing. Two independent reasons to distrust
    /// a size are two reasons, and a scheme that took the most conservative
    /// single rule would let the second one cost nothing.
    ///
    /// Compounds the rows every plane holds — the causal graph and the
    /// belief state, both shipped to a cell in the policy payload. The
    /// self-model row is not here: a cell never holds a self-model, so its
    /// table reads that row as unavailable by default rather than by
    /// measurement, and compounding it would move every cell's floor on a
    /// number nobody computed. The centre, which does hold one, sizes by
    /// [`Self::central_sizing_multiplier`].
    pub fn sizing_multiplier(&self) -> Decimal {
        let mut multiplier = Decimal::from_int(1);
        if !self.freshness(Capability::CausalGraph).is_fresh() {
            multiplier = Self::scaled(CAUSAL_STALE_MULTIPLIER, multiplier);
        }
        if !self.freshness(Capability::BeliefState).is_fresh() {
            multiplier = Self::scaled(BELIEF_STALE_MULTIPLIER, multiplier);
        }
        multiplier
    }

    /// The multiplier the centre applies to size: [`Self::sizing_multiplier`]
    /// compounded with §6.2 row 6, the self-model.
    ///
    /// Fails closed like every other reading here: a centre that never
    /// observed [`Capability::SelfModel`] gets the unavailable multiplier,
    /// because a self-model nobody consulted is indistinguishable, for
    /// sizing, from one that does not exist. Stale and unavailable narrow by
    /// different amounts — see [`SELF_MODEL_STALE_MULTIPLIER`] and
    /// [`SELF_MODEL_UNAVAILABLE_MULTIPLIER`] — which is the distinction
    /// [`Freshness`] is three-valued to keep.
    pub fn central_sizing_multiplier(&self) -> Decimal {
        let shared = self.sizing_multiplier();
        match self.freshness(Capability::SelfModel) {
            Freshness::Fresh => shared,
            Freshness::Stale => Self::scaled(SELF_MODEL_STALE_MULTIPLIER, shared),
            Freshness::Unavailable => Self::scaled(SELF_MODEL_UNAVAILABLE_MULTIPLIER, shared),
        }
    }

    /// Apply a policy constant, narrowing further if it cannot be represented.
    ///
    /// The fallback is deliberately asymmetric. If the constant or the product
    /// cannot be represented we return zero rather than the un-narrowed
    /// multiplier, because the instruction that governs every branch here is
    /// that an unrepresentable state narrows further and never less.
    fn scaled(constant: (i128, u32), current: Decimal) -> Decimal {
        let zero = Decimal::from_int(0);
        let Some(factor) = Decimal::from_scaled(constant.0, constant.1) else {
            return zero;
        };
        current.checked_mul(factor).unwrap_or(zero)
    }

    /// `scaled`, reachable from a test.
    ///
    /// Both fallible branches above are unreachable through
    /// [`Self::sizing_multiplier`], because the only constants it applies are
    /// 0.75 and 0.5 against a starting 1 and neither can fail. That made the
    /// module's central safety claim — that an unrepresentable state narrows
    /// further and never less — an assertion in prose with nothing exercising
    /// it: inverting `unwrap_or(zero)` to `unwrap_or(current)`, so that a
    /// failed multiply *widens*, left the whole suite green.
    ///
    /// This exists so the claim is checked rather than merely made.
    #[cfg(test)]
    fn scaled_for_test(constant: (i128, u32), current: Decimal) -> Decimal {
        Self::scaled(constant, current)
    }

    /// How capital is spread, given what can still be reasoned about.
    pub fn allocation_mode(&self) -> AllocationMode {
        if self.freshness(Capability::CausalGraph).is_fresh() {
            AllocationMode::RegimeConditional
        } else {
            AllocationMode::Unconditional
        }
    }

    /// Whether the platform halts.
    ///
    /// Always false, and it is a method rather than an absence so that the
    /// property is testable and so that a future change that wants to halt has
    /// to come through here and explain itself. §6.2's entire premise is that
    /// the platform narrows rather than stopping; halting belongs to the kill
    /// switch, which is a control an operator holds, not a consequence of a
    /// warm-path job being late.
    pub const fn halts(&self) -> bool {
        false
    }

    /// Every capability that is not fresh, in a deterministic order.
    pub fn narrowed(&self) -> BTreeMap<Capability, Freshness> {
        Capability::all()
            .into_iter()
            .map(|capability| (capability, self.freshness(capability)))
            .filter(|(_, freshness)| !freshness.is_fresh())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multiplier_that_cannot_be_represented_narrows_to_nothing() {
        // The fail-closed arm, which no path through `sizing_multiplier` can
        // reach. The asymmetry is the point: when a constant or a product
        // cannot be represented we return zero, not the un-narrowed
        // multiplier, because every branch here narrows further and never
        // less.
        let one = Decimal::from_int(1);

        // Premise: a representable constant does *not* return zero, so the
        // assertions below are about the failure and not about the function
        // always returning zero.
        let ordinary = DegradationState::scaled_for_test((75, 2), one);
        assert!(
            !ordinary.is_zero(),
            "a representable constant returned zero"
        );

        // An exponent no decimal of this scale can carry.
        let unrepresentable = DegradationState::scaled_for_test((1, u32::MAX), one);
        assert!(
            unrepresentable.is_zero(),
            "an unrepresentable constant widened instead of narrowing: \
             {unrepresentable:?}"
        );

        // And the multiply itself, driven past what the type can hold. Both
        // operands must be *representable* or the `from_scaled` branch above
        // catches it first and this arm stays untested — which is exactly what
        // happened on the first attempt: `from_scaled(i128::MAX, 0)` overflows
        // its own scaling and returns `None`, so the case was skipped and
        // inverting `unwrap_or(zero)` to `unwrap_or(current)` left the suite
        // green.
        //
        // `Decimal` scales by 10^9, so a mantissa near 10^28 is representable
        // while the square of one is not.
        let big = 10i128.pow(28);
        let representable =
            Decimal::from_scaled(big, 0).expect("10^28 is within the decimal's range");
        assert!(
            !representable.is_zero(),
            "the premise failed: the operand is not representable, so the \
             multiply below is not the thing being tested"
        );
        let overflowed = DegradationState::scaled_for_test((big, 0), representable);
        assert!(
            overflowed.is_zero(),
            "an overflowing multiply widened instead of narrowing: {overflowed:?}"
        );
    }
}
