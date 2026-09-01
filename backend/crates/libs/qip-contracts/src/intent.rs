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
//! §27.2 says a cycle leg is never netted — see [`CycleLeg`], which is the
//! only thing that can produce a no-net intent, and [`NettingPolicy::NoNet`].

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
/// cycle still executes, at sizes that no longer close. [`net`] gives every
/// no-net intent its own group, so no leg can join a directional net.
///
/// **What holds this, precisely.** The policy is a private field of
/// [`Intent`] with no setter. [`Intent::new`] always yields [`Self::Nettable`]
/// and is the only way to build a directional intent; [`CycleLeg::new`]
/// always yields [`Self::NoNet`] and, through `From<CycleLeg>`, is the only
/// way to obtain an [`Intent`] carrying one. There is no builder method that
/// flips one into the other, so the failure an earlier version of this type
/// permitted — a leg producer that forgot to call a marking method and had
/// its leg netted against directional flow into an order that was well-formed,
/// plausibly sized and wrong, with nothing in the journal to notice — is not
/// expressible. A producer of legs returns `Vec<CycleLeg>`, and the type of
/// its return value says so.
///
/// The one path that bypasses the constructors is `Deserialize` on
/// [`Intent`], which will read whatever policy the wire carries. Nothing in
/// the tree deserialises an intent today; intents are made in-process, netted
/// in-process, and only the resulting [`NetIntent`] is recorded. That is the
/// boundary of the guarantee, and it is stated here so that the first thing to
/// read an intent off a wire knows it has to check.
///
/// **What produces a leg today.** `qip_arbitrage::Opportunity::cycle_legs`
/// turns a planned cycle into `Vec<CycleLeg>`. It is called by that crate's
/// tests and by nothing deployed: a placement audit found the scanner is
/// constructed by no composition root, and the leg coordinator that would
/// execute a cycle, `qip_execution_engine::multileg::LegGroup`, has zero call
/// sites. Wiring the scanner into the cell's intent seam is the next slice.
/// This refusal therefore guards a path the type system closes and nothing
/// yet walks — which is the right order: the guard has to be true on the day
/// the producer is wired, not added to a netting engine that has been quietly
/// combining legs with directional intents in the meantime.
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
    /// Private, with no setter: see [`NettingPolicy`] for what that holds.
    netting: NettingPolicy,
    /// The feature revisions this intent was reasoned from, carried straight
    /// off the signal so a fill can be attributed to exactly the values that
    /// produced it rather than to whatever those features say by the time
    /// somebody looks.
    ///
    /// Not "hypotheses". The central plane has a hypothesis vocabulary —
    /// `Order::hypotheses`, set from a proposal leg — and the edge has none: a
    /// cell's strategies emit `Signal`s, which name their inputs and no claim.
    /// This field carried the name `hypotheses` for exactly one commit and
    /// nothing could ever populate it, which is the shape of a control that
    /// cannot fire.
    pub inputs: Vec<(String, u64)>,
    pub valid_until: Timestamp,
}

impl Intent {
    /// Build a **directional** intent, refusing a size of zero.
    ///
    /// Always [`NettingPolicy::Nettable`]. A cycle leg is not built here; it
    /// is built by [`CycleLeg::new`], which is the only constructor that
    /// yields a no-net intent.
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
            inputs: Vec::new(),
            valid_until,
        })
    }

    pub fn with_representation(mut self, representation: Representation) -> Self {
        self.representation = representation;
        self
    }

    /// Carry the signal's feature revisions onto the intent.
    pub fn with_inputs(mut self, inputs: Vec<(String, u64)>) -> Self {
        self.inputs = inputs;
        self
    }

    /// Whether this intent may be netted, and with which cycle it travels if
    /// not. Read-only: the policy is fixed by whichever constructor made it.
    pub const fn netting(&self) -> &NettingPolicy {
        &self.netting
    }

    /// The cycle this intent is a leg of, or `None` for a directional intent.
    pub fn cycle_id(&self) -> Option<&str> {
        match &self.netting {
            NettingPolicy::Nettable => None,
            NettingPolicy::NoNet { cycle_id } => Some(cycle_id),
        }
    }

    /// The absolute size, for the gross total.
    pub fn gross(&self) -> Decimal {
        self.signed_size.abs()
    }
}

/// One leg of an arbitrage cycle, which cannot be built nettable.
///
/// This is a distinct type rather than a flag on [`Intent`] because the
/// guarantee §27.2 asks for is about what a *producer* hands back, and a
/// producer's promise lives in its return type. A scanner that returns
/// `Vec<CycleLeg>` cannot return a leg that forgot to say it was one: there is
/// no way to spell such a value. The conversion into [`Intent`] — the one
/// shape the cell's netting seam accepts — is `From<CycleLeg>`, and it hands
/// over the intent this type has been holding since construction, whose
/// policy was fixed at [`NettingPolicy::NoNet`] by [`Self::new`] and has no
/// setter anywhere.
///
/// The two things this makes inexpressible, each a `compile_fail` doctest so
/// the claim is checked rather than asserted. Each names the one field it
/// guards, because a `compile_fail` block passes on *any* error: the first
/// stops at `leg.intent` — `CycleLeg::intent` is private and the accessor is
/// read-only — and never reaches `netting`, so it proves nothing about
/// `netting` and does not claim to. The second reaches `netting` directly on
/// a directional intent and is the one that holds that field private. An
/// earlier version of this comment described the first block as guarding
/// `netting`; it never did.
///
/// ```compile_fail,E0616
/// # use qip_contracts::intent::{CycleLeg, Intent, NettingPolicy};
/// # use qip_contracts::signal::StrategyId;
/// # use qip_contracts::venue::VenueId;
/// # use qip_core::{ObjectId, Timestamp, dec};
/// // A leg's intent cannot be reached to be changed at all: `CycleLeg::intent`
/// // is private and `CycleLeg::intent()` hands out a shared reference. This
/// // fails on `leg.intent`, before `netting` is ever looked at.
/// let mut leg = CycleLeg::new(
///     "cycle-7", StrategyId::new("arb"), ObjectId::from_string("ACME"),
///     VenueId::new("XLON"), dec!("50"), dec!("100"), Timestamp::from_secs(1),
/// ).unwrap();
/// let intent: &mut Intent = &mut leg.intent;
/// intent.netting = NettingPolicy::Nettable;
/// ```
///
/// ```compile_fail,E0616
/// # use qip_contracts::intent::{Intent, NettingPolicy};
/// # use qip_contracts::signal::StrategyId;
/// # use qip_contracts::venue::VenueId;
/// # use qip_core::{ObjectId, Timestamp, dec};
/// // And a directional intent cannot be flipped the other way either:
/// // `Intent::netting` is private, so it is not assignable after
/// // construction. This is the block that guards `netting`.
/// let mut intent = Intent::new(
///     StrategyId::new("arb"), ObjectId::from_string("ACME"), VenueId::new("XLON"),
///     dec!("50"), dec!("100"), Timestamp::from_secs(1),
/// ).unwrap();
/// intent.netting = NettingPolicy::NoNet { cycle_id: "cycle-7".to_string() };
/// ```
///
/// Not `Deserialize`: a leg read off a wire could carry any policy, and the
/// point of the type is that it cannot. Legs are made in-process by the
/// scanner and become intents before anything is recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct CycleLeg {
    intent: Intent,
}

impl CycleLeg {
    /// Build a leg of the named cycle, refusing an unnamed cycle and a size of
    /// zero.
    ///
    /// The cycle id is refused when empty because it is what [`net`] isolates
    /// on and what the journal identifies the atomic set by; a leg of the
    /// cycle called `""` is a leg of nothing anyone can find afterwards.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cycle_id: impl Into<String>,
        strategy: StrategyId,
        object_id: ObjectId,
        venue: VenueId,
        signed_size: Decimal,
        reference_price: Decimal,
        valid_until: Timestamp,
    ) -> qip_core::error::Result<Self> {
        let cycle_id = cycle_id.into();
        if cycle_id.trim().is_empty() {
            return Err(qip_core::error::Error::invalid(
                "a cycle leg must name its cycle: the id is what keeps the leg out of every \
                 directional net and what the journal identifies the atomic set by, and an \
                 empty one names nothing",
            ));
        }
        let mut intent = Intent::new(
            strategy,
            object_id,
            venue,
            signed_size,
            reference_price,
            valid_until,
        )?;
        intent.netting = NettingPolicy::NoNet { cycle_id };
        Ok(Self { intent })
    }

    pub fn with_representation(mut self, representation: Representation) -> Self {
        self.intent.representation = representation;
        self
    }

    /// Carry the scan's inputs onto the leg, as a signal's revisions are
    /// carried onto a directional intent.
    pub fn with_inputs(mut self, inputs: Vec<(String, u64)>) -> Self {
        self.intent.inputs = inputs;
        self
    }

    /// The cycle this leg belongs to. Never empty.
    pub fn cycle_id(&self) -> &str {
        match &self.intent.netting {
            NettingPolicy::NoNet { cycle_id } => cycle_id,
            // Unreachable by construction — `new` is the only constructor and
            // it sets `NoNet` — but the match is exhaustive rather than a
            // panic, so a future constructor that broke the invariant would
            // fail a test on this value rather than abort a cell.
            NettingPolicy::Nettable => "",
        }
    }

    /// The intent this leg will become, read-only.
    pub const fn intent(&self) -> &Intent {
        &self.intent
    }
}

impl From<CycleLeg> for Intent {
    fn from(leg: CycleLeg) -> Self {
        leg.intent
    }
}

/// One strategy's share of a net intent, and the record that makes a fill
/// traceable back to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contributor {
    pub strategy: StrategyId,
    /// Signed, so the vector sums to the net rather than to the gross.
    pub signed_size: Decimal,
    /// The feature revisions behind this contributor's share, from its own
    /// signal. Attribution after the fact needs the inputs *this* strategy
    /// reasoned from, not the union across the net.
    pub inputs: Vec<(String, u64)>,
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
///
/// **Built by [`net`] and by nothing else.** The `sealed` field is private and
/// its type is not nameable outside this module, so a struct literal and
/// struct-update syntax both fail to compile anywhere else — which is what
/// makes "a cycle leg is never netted with a directional intent" a property
/// of the type rather than of one function: the only vector of contributors
/// that can exist under a `cycle_id` is the one [`net`] assembled, and [`net`]
/// gives every leg a group of its own.
///
/// Two `compile_fail` doctests hold this, and each spells out the `sealed`
/// field rather than omitting it. An earlier version left `sealed:` out of
/// the literal, so it failed on "missing field" whatever the field's
/// visibility — a security review made both `sealed` and `Sealed` `pub` and
/// the doctest kept passing. A `compile_fail` block passes on any error, so
/// it guards only what its one error is about; these two are written so the
/// only error left is the privacy of the seal, and they compile — and fire —
/// the moment it is opened.
///
/// The literal, which needs both the field and its type to be reachable:
///
/// ```compile_fail,E0603
/// # use qip_contracts::intent::{NetIntent, Representation, Sealed};
/// # use qip_contracts::venue::VenueId;
/// # use qip_core::{ObjectId, dec};
/// // Every field is supplied, `sealed` included. The only thing wrong with
/// // this literal is that `Sealed` cannot be named from here and `sealed`
/// // cannot be written from here: a caller cannot assemble a net that puts
/// // directional contributors under a `cycle_id` without going through
/// // `net`, which will not.
/// let forged = NetIntent {
///     sealed: Sealed,
///     object_id: ObjectId::from_string("ACME"),
///     venue: VenueId::new("XLON"),
///     representation: Representation::Spot,
///     net_size: dec!("1"),
///     gross_size: dec!("1"),
///     contributors: Vec::new(),
///     reference_price: dec!("100"),
///     cycle_id: Some("cycle-7".to_string()),
/// };
/// ```
///
/// And struct-update from a net that [`net`] really built, which never names
/// the type at all — so a `pub sealed: Sealed` with `Sealed` left private
/// would still open this route, and this block is what fires on it:
///
/// ```compile_fail,E0451
/// # use qip_contracts::intent::{Intent, NetIntent, net};
/// # use qip_contracts::signal::StrategyId;
/// # use qip_contracts::venue::VenueId;
/// # use qip_core::{ObjectId, Timestamp, dec};
/// let genuine = net(vec![Intent::new(
///     StrategyId::new("alpha"), ObjectId::from_string("ACME"), VenueId::new("XLON"),
///     dec!("50"), dec!("100"), Timestamp::from_secs(1),
/// ).unwrap()]).pop().unwrap();
/// // A directional net re-labelled as a cycle: the seal is copied across by
/// // `..genuine`, and copying a private field out of another module is
/// // refused.
/// let forged = NetIntent { cycle_id: Some("cycle-7".to_string()), ..genuine };
/// ```
///
/// What this does **not** hold, stated so nobody reads more into it: the
/// remaining fields are public because the cell reads them directly, and a
/// caller that owns a `NetIntent` can still assign to them. Closing that means
/// private fields with accessors, which touches every read in `qip-edge`'s
/// cell and belongs to that crate's owner; the seal on construction is the
/// half that the netting review found open. `Deserialize` is the other path,
/// and `sealed` is skipped on the wire, so a net read back from a journal is a
/// record of one `net` built and not a fresh one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetIntent {
    #[serde(skip)]
    sealed: Sealed,
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

/// The token that makes [`NetIntent`] constructible only in this module.
///
/// Zero-sized and private: it costs nothing at runtime and cannot be named
/// from outside, so it cannot be supplied to a literal from outside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sealed;

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
    /// The remainder is settled in both directions, and the second direction
    /// is not hypothetical. `Decimal` holds nine decimal places and its
    /// multiply and divide round half away from zero at the ninth, while the
    /// shares here are floored at the eighth. When that rounding lifts a share
    /// to a value already exact at eight places, the floor takes nothing back
    /// — and if it does so for every contributor, the shares sum to *more*
    /// than the fill. Splitting `0.100000019` between two equal contributors
    /// does exactly that, and `0.100000019` is an ordinary crypto quantity
    /// rather than contrived dust. This function previously tested only
    /// `remainder.is_positive()`, so the excess was silently dropped: envelopes
    /// were charged for notional nobody traded, and attribution carried a
    /// residual of the precise sign [`Attribution::reconciles`] exists to
    /// forbid.
    ///
    /// A shortfall is handed out to the largest remainders first, ties broken
    /// by strategy id ascending. An excess is taken back from the smallest
    /// remainders first, ties by strategy id descending — the mirror image, so
    /// that whichever way the rounding went the same fill splits the same way
    /// on every machine and in every replay.
    ///
    /// [`net`] pre-sorts contributors by strategy id, so a vector that came
    /// straight from it reaches the tie-breaks already in the order they
    /// would impose. That is not every vector this sees. `contributors` is a
    /// public field a caller can reorder, and [`NetIntent`] derives
    /// `Deserialize` with only `sealed` skipped, so a net read off a journal
    /// or a wire arrives in whatever order the bytes carry — a code review
    /// deserialised one from a JSON literal, contributors reversed, without
    /// complaint. For those callers the tie-breaks below are the only thing
    /// standing between equal remainders and arrival order, and each is held
    /// by a test that reorders the vector first. An earlier version of this
    /// comment said the seal made every vector a sorted one and the tie-breaks
    /// therefore untestable; the give-back tie-break was in fact already held
    /// by such a test, and the hand-out one was not held by any.
    ///
    /// The open item is `Deserialize` itself: a net read back from bytes is
    /// a net whose contributor vector nobody in-process assembled, and whether
    /// `NetIntent` should be readable from the wire at all is a design
    /// question, not something a comment or a tie-break closes. Nothing in the
    /// tree deserialises one today.
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
            allocated += floored;
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
                shares[index].1 += step;
                remainder -= step;
            }
        } else if remainder.is_negative() {
            // The mirror: ascending remainder, then strategy id descending, so
            // the contributor that gained least from the flooring is the first
            // to give the excess back.
            //
            // One pass is enough, and the bound is worth stating because a
            // second pass would be unreachable code pretending to be a
            // safeguard. Only a share the ninth-place rounding lifted can
            // contribute to an excess, and it can contribute less than one
            // unit at the eighth place; any such share is itself at least one
            // unit at the eighth place, because it survived the floor as
            // non-zero. So each non-zero share can absorb a full step, and
            // there are at least as many of them as there are steps to take.
            let mut order: Vec<usize> = (0..shares.len()).collect();
            order.sort_by(|left, right| {
                shares[*left]
                    .2
                    .cmp(&shares[*right].2)
                    .then_with(|| shares[*right].0.as_str().cmp(shares[*left].0.as_str()))
            });
            let unit = UNIT;
            let mut owed = Decimal::ZERO - remainder;
            for index in order {
                if !owed.is_positive() {
                    break;
                }
                // A share of nothing cannot give anything back, and taking
                // from it would hand a contributor a negative share of a fill
                // it never received.
                if !shares[index].1.is_positive() {
                    continue;
                }
                let step = if owed < unit { owed } else { unit };
                shares[index].1 -= step;
                owed -= step;
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
                    inputs: intent.inputs,
                })
                .collect();
            NetIntent {
                sealed: Sealed,
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
