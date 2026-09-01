//! What a strategy wants done, and what many strategies wanting things becomes.
//!
//! Blueprint §27 states the invariant flatly: **no strategy sends an order.**
//! Strategies produce intents; intents are netted; the net is gated; one order
//! goes out; the fill is attributed back pro-rata. Without that, a cell with
//! two strategies buying the same instrument at the same venue sends two
//! orders — which pays the spread twice, telegraphs the position, and lets one
//! strategy's buy cross another's sell. That last one is a self-trade: a
//! regulatory problem and a pure loss at the same time, and it is what this
//! module exists to make structurally impossible.
//!
//! # Why signed sizes rather than a side and a magnitude
//!
//! Netting is addition. A side-plus-magnitude pair turns cancellation into a
//! conditional, and a conditional is a thing somebody can get backwards; a
//! signed sum cannot be. Positive buys, negative sells, and two opposing
//! intents of equal size produce a net of exactly zero — which is not an error
//! but the outcome the whole mechanism exists to produce.
//!
//! # What this module is not
//!
//! It is not [`crate::signal::Signal`], which is the strategy's own view
//! before any venue is chosen or any capital check has run. An [`Intent`] is
//! *derived from* a signal by something that knows the venue and has already
//! admitted the size against a capital envelope. And it is not a leg group:
//! a leg group exists so that legs complete **together**, while netting exists
//! so that intents **collapse**. The two are opposites, which is exactly why
//! §27.2 says a cycle leg is never netted — see [`NettingPolicy::NoNet`].

use crate::signal::StrategyId;
use crate::venue::VenueId;
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which representation of an underlying an intent trades.
///
/// §27.2: spot and perpetual are different instruments with different risk and
/// are never netted, even when the underlying matches. Exposure is aggregated
/// for risk; the orders stay separate.
///
/// **This is a control that cannot fire today, and it is recorded as such
/// rather than presented as protection.** Instruments in this repository are
/// identified by [`ObjectId`] alone and carry no representation, so every
/// intent is built as [`Self::Spot`] and the guard in [`net`] has nothing to
/// separate. It ships now because the netting key is the thing that would have
/// to change later — adding it afterwards means re-deriving every key already
/// written to a journal — and it becomes live the moment instruments carry a
/// representation. Until then, the row it protects is empty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
    #[default]
    Spot,
    Perpetual,
    Future,
}

impl Representation {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::Perpetual => "perpetual",
            Self::Future => "future",
        }
    }
}

/// Whether an intent may be combined with others.
///
/// §27.2: an arbitrage cycle leg is part of an atomic set, and netting it
/// against a directional intent silently breaks the cycle's economics — the
/// cycle still executes, at sizes that no longer close. The refusal is
/// structural rather than advisory: [`net`] gives every no-net intent its own
/// group, so there is no code path on which a leg joins a directional net.
///
/// **Nothing produces a cycle leg today, and the honest statement is stronger
/// than "not wired yet".** A placement audit of the node's composition roots
/// found that `qip-arbitrage` — the executable graph, the cycle scanner, the
/// path router — is referenced from the node tree exactly once, and only for
/// a trait import in `qip-edge`'s liquidity seam; `Cell::work` runs compiled
/// strategy programs and never calls the scanner. The leg coordinator that
/// would execute a cycle, `qip_execution_engine::multileg::LegGroup`, has
/// **zero call sites anywhere in the workspace, including tests**. So this
/// refusal currently guards a path nothing walks.
///
/// It ships anyway, and the reason is the ordering rather than the coverage: a
/// guard that refuses has to already be true on the day the producer arrives,
/// because the alternative is adding it afterwards to a netting engine that
/// has been quietly combining legs with directional intents in the meantime.
/// That is a guard, not scaffolding — but it is not protection of a live path
/// either, and it must not be read as one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum NettingPolicy {
    /// May be combined with other nettable intents on the same key.
    Nettable,
    /// Must be executed alone, as part of the named cycle.
    NoNet { cycle_id: String },
}

impl NettingPolicy {
    pub const fn is_nettable(&self) -> bool {
        matches!(self, Self::Nettable)
    }
}

/// What one strategy wants done, after its own capital check and before
/// netting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub strategy: StrategyId,
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// Positive buys, negative sells. Never zero — [`Self::new`] refuses it,
    /// because an intent to trade nothing is a caller confusion and admitting
    /// it would put an empty contributor into a vector that must sum.
    pub signed_size: Decimal,
    /// The price the size was reasoned at, carried for the net's reference and
    /// for slippage measurement afterwards.
    pub reference_price: Decimal,
    pub representation: Representation,
    pub netting: NettingPolicy,
    /// The hypotheses this intent expresses, so a fill can be explained.
    pub hypotheses: Vec<String>,
    pub valid_until: Timestamp,
}

impl Intent {
    /// Build an intent, refusing a size of zero.
    pub fn new(
        strategy: StrategyId,
        object_id: ObjectId,
        venue: VenueId,
        signed_size: Decimal,
        reference_price: Decimal,
        valid_until: Timestamp,
    ) -> qip_core::error::Result<Self> {
        if signed_size.is_zero() {
            return Err(qip_core::error::Error::invalid(
                "an intent of zero size trades nothing and would contribute nothing to a net; \
                 refuse it here rather than carry an empty contributor into a vector that must \
                 sum to the net",
            ));
        }
        Ok(Self {
            strategy,
            object_id,
            venue,
            signed_size,
            reference_price,
            representation: Representation::Spot,
            netting: NettingPolicy::Nettable,
            hypotheses: Vec::new(),
            valid_until,
        })
    }

    pub fn with_representation(mut self, representation: Representation) -> Self {
        self.representation = representation;
        self
    }

    /// Mark this intent as a leg of the named cycle, never to be netted.
    pub fn as_cycle_leg(mut self, cycle_id: impl Into<String>) -> Self {
        self.netting = NettingPolicy::NoNet {
            cycle_id: cycle_id.into(),
        };
        self
    }

    pub fn with_hypotheses(mut self, hypotheses: Vec<String>) -> Self {
        self.hypotheses = hypotheses;
        self
    }

    /// The absolute size, for the gross total.
    pub fn gross(&self) -> Decimal {
        self.signed_size.abs()
    }
}

/// One strategy's share of a net intent, and the record that makes a fill
/// traceable back to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contributor {
    pub strategy: StrategyId,
    /// Signed, so the vector sums to the net rather than to the gross.
    pub signed_size: Decimal,
    pub hypotheses: Vec<String>,
}

/// The key intents are grouped on.
///
/// §27.2 in three fields. Same instrument and same venue nets; a second venue
/// is a different execution at a different price and does not; a different
/// representation of the same underlying is a different instrument with
/// different risk and does not. Ordered, because the grouping reaches the
/// journal and the wire and a replay that reorders is not a replay.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NettingKey {
    object_id: ObjectId,
    venue: VenueId,
    representation: Representation,
    /// `None` for nettable intents; a unique discriminator for each no-net
    /// intent, which is what gives a cycle leg a group of its own rather than
    /// a place in somebody else's.
    isolated: Option<String>,
}

/// N intents collapsed into the one order that will be sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetIntent {
    pub object_id: ObjectId,
    pub venue: VenueId,
    pub representation: Representation,
    /// The sum of every contributor's signed size. **May be zero**, which is
    /// the self-trade case: two strategies wanted opposite things, they
    /// cancelled internally, and nothing should reach the venue.
    pub net_size: Decimal,
    /// The sum of every contributor's absolute size — the numerator of the
    /// netting ratio, and what an internal cross is capped against.
    pub gross_size: Decimal,
    /// Every strategy that wanted something here, in a deterministic order.
    pub contributors: Vec<Contributor>,
    /// The reference price of the largest contributor by absolute size, ties
    /// broken by strategy id. Deterministic, and the price the net was
    /// reasoned at rather than a fresh read.
    pub reference_price: Decimal,
    /// Set when this net came from a no-net intent, so a cycle leg stays
    /// identifiable all the way to the order.
    pub cycle_id: Option<String>,
}

impl NetIntent {
    /// Whether this net produces an order at all.
    ///
    /// A net of zero is a complete internal cancellation. It is a successful
    /// outcome, not a refusal, and the caller records it rather than placing
    /// anything.
    pub fn is_cancelled(&self) -> bool {
        self.net_size.is_zero()
    }

    /// Which way the net order goes, or `None` when it cancels.
    pub fn is_buy(&self) -> Option<bool> {
        if self.net_size.is_zero() {
            None
        } else {
            Some(self.net_size.is_positive())
        }
    }

    /// The size to send, unsigned.
    pub fn order_quantity(&self) -> Decimal {
        self.net_size.abs()
    }

    /// Split `filled` across the contributors, pro-rata on absolute size,
    /// summing **exactly** to `filled`.
    ///
    /// Largest-remainder, deterministically. Truncating division loses a
    /// fraction of every fill and floating point invents one; either way the
    /// shares stop summing to what was actually traded, and unexplained P&L is
    /// precisely what exact attribution exists to make impossible.
    ///
    /// The remainder goes to the largest remainders first, ties broken by
    /// strategy id ascending — so the same fill splits the same way on every
    /// machine and in every replay.
    pub fn split_fill(&self, filled: Decimal) -> Vec<(StrategyId, Decimal)> {
        let denominator = self.gross_size;
        if self.contributors.is_empty() || denominator.is_zero() || filled.is_zero() {
            return Vec::new();
        }
        // Floor each share at the decimal's own scale, then hand out what the
        // flooring left over.
        let mut shares: Vec<(StrategyId, Decimal, Decimal)> = Vec::new();
        let mut allocated = Decimal::ZERO;
        for contributor in &self.contributors {
            let exact = contributor
                .signed_size
                .abs()
                .checked_mul(filled)
                .and_then(|numerator| numerator.checked_div(denominator))
                .unwrap_or(Decimal::ZERO);
            let floored = exact.truncate_dp(SPLIT_SCALE);
            allocated = allocated + floored;
            shares.push((contributor.strategy.clone(), floored, exact - floored));
        }
        let mut remainder = filled - allocated;
        if remainder.is_positive() {
            // Descending remainder, then strategy id ascending. Both halves
            // matter: without the second, two equal remainders would be
            // separated by whatever order the contributors happened to arrive
            // in, and the same fill would split differently across machines.
            let mut order: Vec<usize> = (0..shares.len()).collect();
            order.sort_by(|left, right| {
                shares[*right]
                    .2
                    .cmp(&shares[*left].2)
                    .then_with(|| shares[*left].0.as_str().cmp(shares[*right].0.as_str()))
            });
            let unit = UNIT;
            for index in order {
                if !remainder.is_positive() {
                    break;
                }
                let step = if remainder < unit { remainder } else { unit };
                shares[index].1 = shares[index].1 + step;
                remainder = remainder - step;
            }
        }
        shares
            .into_iter()
            .map(|(strategy, share, _)| (strategy, share))
            .collect()
    }
}

/// The scale shares are floored to before the remainder is distributed.
///
/// One below the decimal's own scale, so a whole number of units is always
/// left to hand out and the loop below terminates.
const SPLIT_SCALE: u32 = 8;

/// The smallest step the remainder is distributed in, at [`SPLIT_SCALE`].
const UNIT: Decimal = Decimal::from_raw(10);

/// The netting ratio: gross intent over net order volume.
///
/// §27: the single best summary of whether a strategy set has genuine
/// diversity. One means every strategy wanted the same thing and nothing
/// cancelled; a large ratio means most of the intent never reached a venue.
/// `None` when the net is zero, because the ratio is unbounded there and
/// reporting a sentinel would put a number nobody computed onto a chart.
pub fn netting_ratio(nets: &[NetIntent]) -> Option<f64> {
    let gross: Decimal = nets
        .iter()
        .map(|net| net.gross_size)
        .fold(Decimal::ZERO, |a, b| a + b);
    let net: Decimal = nets
        .iter()
        .map(NetIntent::order_quantity)
        .fold(Decimal::ZERO, |a, b| a + b);
    if net.is_zero() {
        return None;
    }
    Some(gross.to_f64() / net.to_f64())
}

/// Collapse intents into net intents, per §27.2.
///
/// Grouping is by instrument, venue and representation; a no-net intent gets a
/// group of its own so it can never be combined with a directional one. The
/// result is ordered by the grouping key, so two runs over the same intents
/// produce the same nets in the same order.
pub fn net(intents: Vec<Intent>) -> Vec<NetIntent> {
    let mut groups: BTreeMap<NettingKey, Vec<Intent>> = BTreeMap::new();
    for (index, intent) in intents.into_iter().enumerate() {
        let isolated = match &intent.netting {
            NettingPolicy::Nettable => None,
            // The index makes each no-net intent its own group even when two
            // legs of the same cycle share an instrument and a venue: they are
            // separate legs of an atomic set and combining them is the same
            // mistake as netting them with a directional intent.
            NettingPolicy::NoNet { cycle_id } => Some(format!("{cycle_id}#{index}")),
        };
        let key = NettingKey {
            object_id: intent.object_id.clone(),
            venue: intent.venue.clone(),
            representation: intent.representation,
            isolated,
        };
        groups.entry(key).or_default().push(intent);
    }

    groups
        .into_iter()
        .map(|(key, mut members)| {
            // Deterministic contributor order: strategy id ascending. The
            // vector reaches the journal and the wire.
            members.sort_by(|left, right| left.strategy.as_str().cmp(right.strategy.as_str()));
            let net_size = members
                .iter()
                .map(|intent| intent.signed_size)
                .fold(Decimal::ZERO, |a, b| a + b);
            let gross_size = members
                .iter()
                .map(Intent::gross)
                .fold(Decimal::ZERO, |a, b| a + b);
            let reference_price = members
                .iter()
                .max_by(|left, right| {
                    left.gross()
                        .cmp(&right.gross())
                        .then_with(|| right.strategy.as_str().cmp(left.strategy.as_str()))
                })
                .map_or(Decimal::ZERO, |intent| intent.reference_price);
            let cycle_id = members.iter().find_map(|intent| match &intent.netting {
                NettingPolicy::Nettable => None,
                NettingPolicy::NoNet { cycle_id } => Some(cycle_id.clone()),
            });
            let contributors = members
                .into_iter()
                .map(|intent| Contributor {
                    strategy: intent.strategy,
                    signed_size: intent.signed_size,
                    hypotheses: intent.hypotheses,
                })
                .collect();
            NetIntent {
                object_id: key.object_id,
                venue: key.venue,
                representation: key.representation,
                net_size,
                gross_size,
                contributors,
                reference_price,
                cycle_id,
            }
        })
        .collect()
}
