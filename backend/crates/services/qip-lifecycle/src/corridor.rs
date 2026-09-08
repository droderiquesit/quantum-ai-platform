//! Corridor policy — what the Intelligence layer permits a corridor to carry.
//!
//! Blueprint §2's layer table gives Intelligence one sentence: "Trains models,
//! generates and statistically gates strategies, **sets risk and corridor
//! policy**." The treasury owns corridors as records — `qip-capital-fabric`
//! holds the corridor lifecycle, the destination registry, the seven-veto
//! transfer gate and custody policy as data — but nothing set the policy those
//! records are measured against, and a scorecard row that read "corridor policy
//! has no subject" was describing exactly that: a control with nothing to
//! control.
//!
//! # What this decides, and what it deliberately cannot do
//!
//! A corridor exists to move capital to where a strategy will use it. Whether
//! it may, and how much it may carry, is a question about the strategies it
//! funds, and the lifecycle ledger is the only record of where those stand. So
//! the policy is derived from the **weakest rung any strategy the corridor
//! funds is standing on**:
//!
//! * a corridor funding anything that has been retired, or that is not on a
//!   rung holding capital, is [`CorridorStanding::Suspended`] and carries
//!   nothing;
//! * a corridor whose weakest funded strategy is at [`GateStage::Pilot`] is
//!   [`CorridorStanding::Narrowed`] to the pilot ceiling — pilot is "live with
//!   capital, deliberately limited", and a corridor that keeps its full ceiling
//!   while the strategy behind it is limited has undone the limit;
//! * only a corridor every one of whose strategies has reached
//!   [`GateStage::Scaled`] is [`CorridorStanding::Permitted`] at its full
//!   ceiling.
//!
//! Both ceilings are **stated by an operator**, never computed. The policy
//! selects between two figures a person wrote down; it does not scale, taper or
//! interpolate. A cap this crate invented would be a number with no owner, and
//! the veto it produced would be unarguable for the wrong reason.
//!
//! ADR 0021 bounds the rest, and nothing here approaches it: this module emits
//! a cap and a refusal. It signs nothing, moves nothing and calls nothing out.
//! There is no transfer engine and there may not be one.
//!
//! # Why a suspension is not an error
//!
//! [`CorridorStanding::Suspended`] is the control **working**. It means the
//! platform has stopped routing capital to a strategy that no longer holds any,
//! not that a corridor has failed. An operator who reads it as a fault will go
//! looking for the wrong problem, which is the same trap the observability
//! rules call out for `risk_figure_unevaluated`.

use crate::ledger::LifecycleLedger;
use qip_contracts::gate::GateStage;
use qip_contracts::signal::StrategyId;
use qip_core::Timestamp;
use qip_core::decimal::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Where a corridor runs and in what asset.
///
/// Ordered on the tuple so a policy's rulings iterate in one order for one
/// input, whatever order the subjects were declared in. A replay that reorders
/// is not a replay.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CorridorRoute {
    pub source: String,
    pub destination: String,
    pub asset: String,
}

impl CorridorRoute {
    /// Refuses a blank leg. A corridor missing an endpoint is a policy nobody
    /// can match a transfer against, and an unmatched transfer is one the gate
    /// downstream has no rule for.
    pub fn new(
        source: impl Into<String>,
        destination: impl Into<String>,
        asset: impl Into<String>,
    ) -> Result<Self> {
        let route = Self {
            source: source.into(),
            destination: destination.into(),
            asset: asset.into(),
        };
        for (label, value) in [
            ("source", &route.source),
            ("destination", &route.destination),
            ("asset", &route.asset),
        ] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "a corridor's {label} is blank; name all three of source, destination and \
                     asset, because a transfer is matched to a policy on the whole route"
                )));
            }
        }
        Ok(route)
    }
}

impl std::fmt::Display for CorridorRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} -> {} in {}",
            self.source, self.destination, self.asset
        )
    }
}

/// A corridor the desk has declared, and the strategies it exists to fund.
///
/// `funds` is the subject the scorecard said this capability did not have.
/// [`Self::new`] refuses an empty one, because a corridor that funds nothing
/// has no lifecycle standing to be judged on, and a policy that admits it would
/// permit the full ceiling on evidence about nobody.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorridorSubject {
    route: CorridorRoute,
    ceiling: Decimal,
    pilot_ceiling: Decimal,
    funds: BTreeSet<StrategyId>,
}

impl CorridorSubject {
    /// Refuses a ceiling that is not positive, a pilot ceiling above the full
    /// one, and a corridor funding nobody.
    ///
    /// A pilot ceiling above the full ceiling is refused rather than clamped:
    /// it means whoever wrote the two numbers had them the wrong way round, and
    /// a silent correction would leave that belief in place for the next pair.
    ///
    /// **Zero is refused alongside the negatives, and this is where that
    /// refusal belongs.** A ceiling of zero is not a narrow corridor; it is a
    /// suspension expressed as a cap, and every control downstream reads it as
    /// the former. [`emit`] derives [`CorridorStanding::Narrowed`] (or
    /// `Permitted`) at zero because the rung holds capital,
    /// `CorridorFunding::well_formed` in the fabric sees a non-negative ceiling
    /// under a standing that is not suspended, the transfer gate's check 1
    /// admits the corridor, and check 2 then refuses every transfer ever
    /// proposed with "exceeds the narrowed ceiling of 0" — sending an operator
    /// to promote a strategy when the cause is a zero somebody typed into a
    /// declaration. Refusing it here makes the state unconstructible rather
    /// than detectable: the two honest ways to say "this corridor carries
    /// nothing" are to state a real ceiling and let the rungs narrow it, or to
    /// stop funding the strategy, and both are the declarer's to choose rather
    /// than this crate's to guess.
    pub fn new(
        route: CorridorRoute,
        ceiling: Decimal,
        pilot_ceiling: Decimal,
        funds: impl IntoIterator<Item = StrategyId>,
    ) -> Result<Self> {
        for (label, value) in [("ceiling", ceiling), ("pilot ceiling", pilot_ceiling)] {
            if !value.is_positive() {
                return Err(Error::invalid(format!(
                    "the {label} for the corridor {route} is {value}; a cap is a positive amount \
                     — a corridor that may carry nothing is suspended by where the strategies it \
                     funds stand, and a ceiling of {value} would be published as a policy that \
                     permits the corridor and then refuses every transfer through it"
                )));
            }
        }
        if pilot_ceiling > ceiling {
            return Err(Error::invalid(format!(
                "the corridor {route} has a pilot ceiling of {pilot_ceiling} above its full \
                 ceiling of {ceiling}; a pilot rung is a smaller commitment than a scaled one, so \
                 the two figures are the wrong way round — swap them rather than relying on this \
                 stage to reorder them"
            )));
        }
        let funds: BTreeSet<StrategyId> = funds.into_iter().collect();
        if funds.is_empty() {
            return Err(Error::invalid(format!(
                "the corridor {route} funds no strategy; name the strategies it moves capital for, \
                 because the policy is derived from where they stand and a corridor with no \
                 subject would be permitted its full ceiling on evidence about nobody"
            )));
        }
        Ok(Self {
            route,
            ceiling,
            pilot_ceiling,
            funds,
        })
    }

    pub fn route(&self) -> &CorridorRoute {
        &self.route
    }

    pub fn ceiling(&self) -> Decimal {
        self.ceiling
    }

    pub fn pilot_ceiling(&self) -> Decimal {
        self.pilot_ceiling
    }

    pub fn funds(&self) -> &BTreeSet<StrategyId> {
        &self.funds
    }
}

/// What the Intelligence layer permits this corridor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorridorStanding {
    /// Every strategy it funds has reached scaled. Full ceiling.
    Permitted,
    /// Its weakest funded strategy is at pilot. Pilot ceiling.
    Narrowed,
    /// Something it funds holds no capital. Carries nothing. **This is the
    /// control working, not failing.**
    Suspended,
}

impl CorridorStanding {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Permitted => "permitted",
            Self::Narrowed => "narrowed",
            Self::Suspended => "suspended",
        }
    }
}

impl std::fmt::Display for CorridorStanding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One corridor's ruling, with the strategy that decided it.
///
/// `decided_by` and `decided_at_stage` name the weakest funded strategy rather
/// than summarising the set. A cap an operator cannot trace to one named rung
/// is a cap nobody can argue with, and the corridors this governs are the ones
/// a person has to sign.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorridorRuling {
    pub route: CorridorRoute,
    pub standing: CorridorStanding,
    /// The most this corridor may carry. Zero when suspended.
    pub permitted: Decimal,
    pub decided_by: StrategyId,
    pub decided_at_stage: GateStage,
    pub reason: String,
}

/// The policy as emitted, one ruling per declared corridor.
///
/// Held as a `Vec` ordered by route rather than a map, so the record round
/// trips through JSON with the route whole — a struct is not a map key, and
/// flattening the three legs into one string to make it one would invent a
/// delimiter that a destination name could contain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorridorPolicy {
    emitted_at: Timestamp,
    rulings: Vec<CorridorRuling>,
}

impl CorridorPolicy {
    pub fn emitted_at(&self) -> Timestamp {
        self.emitted_at
    }

    pub fn rulings(&self) -> &[CorridorRuling] {
        &self.rulings
    }

    pub fn ruling_for(&self, route: &CorridorRoute) -> Option<&CorridorRuling> {
        self.rulings.iter().find(|ruling| &ruling.route == route)
    }

    /// Every corridor currently carrying nothing.
    pub fn suspended(&self) -> Vec<&CorridorRuling> {
        self.rulings
            .iter()
            .filter(|ruling| ruling.standing == CorridorStanding::Suspended)
            .collect()
    }

    /// The policy as lines a reviewer can read.
    pub fn narrate(&self) -> Vec<String> {
        self.rulings
            .iter()
            .map(|ruling| {
                format!(
                    "{} is {} at {} — {}",
                    ruling.route, ruling.standing, ruling.permitted, ruling.reason
                )
            })
            .collect()
    }
}

/// Set the corridor policy from where the strategies each corridor funds stand.
///
/// Called by [`LifecycleLedger`] whenever a strategy's standing changes, so the
/// policy is never older than the ledger it is derived from. Refuses two
/// corridors declared on one route: two policies for one route is two claims
/// about the same fact, and a transfer matched against whichever was found
/// first would be governed by a cap nobody chose.
pub fn emit(
    ledger: &LifecycleLedger,
    subjects: &[CorridorSubject],
    now: Timestamp,
) -> Result<CorridorPolicy> {
    let mut seen: BTreeSet<&CorridorRoute> = BTreeSet::new();
    let mut ordered: Vec<&CorridorSubject> = Vec::with_capacity(subjects.len());
    for subject in subjects {
        if !seen.insert(subject.route()) {
            return Err(Error::invalid(format!(
                "the corridor {} is declared twice; give each route one policy, because a \
                 transfer matched against whichever was found first would be governed by a cap \
                 nobody chose",
                subject.route()
            )));
        }
        ordered.push(subject);
    }
    ordered.sort_by(|left, right| left.route().cmp(right.route()));

    let mut rulings = Vec::with_capacity(ordered.len());
    for subject in ordered {
        // The weakest rung decides. `GateStage` is ordered candidate-first with
        // retired last, so "weakest" is not simply a `min`: retired sorts above
        // scaled and is the strongest possible reason to suspend. Take the
        // lowest rung that holds capital, and let anything not holding capital
        // win outright.
        let mut decided_by: Option<(&StrategyId, GateStage)> = None;
        for strategy in subject.funds() {
            let stage = ledger.stage_of(strategy);
            let replaces = match decided_by {
                None => true,
                Some((_, worst)) => {
                    // Not holding capital always beats holding it; among two
                    // that hold capital, the lower rung wins.
                    (!stage.holds_capital() && worst.holds_capital())
                        || (stage.holds_capital() && worst.holds_capital() && stage < worst)
                }
            };
            if replaces {
                decided_by = Some((strategy, stage));
            }
        }
        let (strategy, stage) = decided_by.ok_or_else(|| {
            Error::invalid(format!(
                "the corridor {} reached the policy with no funded strategy; \
                 `CorridorSubject::new` refuses an empty set, so this is a defect in this stage",
                subject.route()
            ))
        })?;

        let (standing, permitted, reason) = if !stage.holds_capital() {
            (
                CorridorStanding::Suspended,
                Decimal::ZERO,
                format!(
                    "{strategy} is at {} and holds no capital, so this corridor has nothing to \
                     fund; this is the policy working, not a corridor fault",
                    stage.as_str()
                ),
            )
        } else if stage == GateStage::Scaled {
            (
                CorridorStanding::Permitted,
                subject.ceiling(),
                format!(
                    "every strategy this corridor funds has reached scaled; the weakest, \
                     {strategy}, is at {}",
                    stage.as_str()
                ),
            )
        } else {
            (
                CorridorStanding::Narrowed,
                subject.pilot_ceiling(),
                format!(
                    "{strategy} is at {}, which is live with capital and deliberately limited; \
                     the corridor is held to the pilot ceiling so the limit is not undone by the \
                     route that funds it",
                    stage.as_str()
                ),
            )
        };

        rulings.push(CorridorRuling {
            route: subject.route().clone(),
            standing,
            permitted,
            decided_by: strategy.clone(),
            decided_at_stage: stage,
            reason,
        });
    }

    Ok(CorridorPolicy {
        emitted_at: now,
        rulings,
    })
}
