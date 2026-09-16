//! `qip-routing` — where an order goes, in what shape, and what happens when
//! the venue says no.
//!
//! Four decisions, each with a way of going wrong that looks fine in a log:
//!
//! * **Which venue.** Comparing quoted prices is comparing the one number that
//!   is not the cost. [`router::Router`] compares venues on the all-in figure —
//!   the sweep, the fee for the side of the book the order will actually be on,
//!   and what the venue's own reject rate costs in re-routing — and records the
//!   quoted price beside it so the two can be seen to disagree.
//! * **What shape.** [`ordertype::select_order_type`] picks between market,
//!   limit, peg, immediate-or-cancel and fill-or-kill from the size against the
//!   displayed liquidity and from how badly the caller needs it done. The
//!   thresholds are the ones `qip-execution-engine` already uses, for the same
//!   reason: a market order in a thin book is how a small position becomes a
//!   large loss.
//! * **What happens to the pieces.** [`children::ParentOrder`] holds the
//!   identity that every share is filled, working, released by a failed child,
//!   or never assigned — and that those four add back up to the parent,
//!   exactly. A child that fails hands its quantity back rather than taking it
//!   with it.
//! * **Whether to keep using a venue.** [`health::HealthTracker`] turns rejects
//!   and latency into a cost that goes into the routing choice, so a degrading
//!   venue is routed away from automatically, and into a verdict with a reason,
//!   so somebody can say why.
//! * **Whether a resting order is still the right order.** [`reprice::Repricer`]
//!   decides when a resting limit child has fallen too far behind the touch,
//!   withdraws it by cancel-plus-new — never amend — and mints the replacement
//!   for the remainder only after the cancel is acknowledged, under requote
//!   budgets that stop it chasing a fast market.
//!
//! * **How a whole cycle will be executed.** [`path`] is blueprint §30.2's
//!   path router: it assigns one of the eight named execution paths from the
//!   classes of a candidate cycle's edges and the facts about the venues they
//!   touch, and refuses a cycle no row of that table covers rather than
//!   picking the nearest one. [`pathcycle::CycleRouter`] reads a cycle the
//!   arbitrage search found. Neither can produce an order; both classify.
//! * **Whether a cross-region side may trade at all.** [`mirror`] is
//!   blueprint §31.1's direction-gating table: a region below its inventory
//!   target may only buy and one above it may only sell, so two regions can
//!   never take the same side of one mirrored asset and neither has to ask
//!   the other. [`extension`] is §33.1's per-path check, one arm per path,
//!   refusing when the fact a row needs is absent. Both are checks: neither
//!   can name a venue, choose a path or raise a size, and a caller that
//!   ignores a refusal from either has skipped a check rather than gained a
//!   permission.
//!
//! * **Whether two strategies that named no venue become one order.**
//!   [`consolidate::Consolidator`] is blueprint §27.2's third row: intents
//!   carrying [`consolidate::UNSPECIFIED_VENUE`] are put onto the venue the
//!   router prices best, *before* netting, so they collapse into one order
//!   instead of crossing each other at two placeholders. It never moves an
//!   intent that named a venue, never changes a size or a side, and an intent
//!   it cannot place is removed and reported rather than sent to a venue that
//!   does not exist.
//!
//! [`gateway::Gateway`] is the venue-facing surface.
//! [`gateway::SimulatedGateway`] implements it against a book;
//! [`gateway::NativeGateway`] is the shape of the real adapter and reports
//! itself unavailable, naming every credential a production deployment would
//! have to supply. Neither pretends: `is_simulated` is on the data, not in the
//! environment.
//!
//! Nothing here reads a clock or a random source it was not handed. `at` is a
//! parameter and the simulator's randomness is seeded, so the same market and
//! the same history route the same way on a replay.

pub mod children;
pub mod consolidate;
pub mod extension;
pub mod gateway;
pub mod health;
pub mod mirror;
pub mod ordertype;
pub mod path;
pub mod pathcycle;
pub mod ratelimit;
pub mod reprice;
pub mod router;
pub mod venue;

pub use children::{ChildOrder, ChildState, ParentOrder};
pub use consolidate::{
    Consolidation, ConsolidationDecision, ConsolidationRefusal, Consolidator, UNSPECIFIED_VENUE,
    is_unspecified,
};
pub use extension::{
    AnchorExtension, BasisExtension, ExtensionVerdict, FirmQuoteExtension, HedgeExtension,
    MirrorExtension, PathExtensions, PayoffExtension, check,
};
pub use gateway::{
    Gateway, GatewayAck, GatewayCredential, GatewayEvent, GatewaySettings, NativeGateway,
    NativeGatewayConfig, SimulatedGateway, WorkingOrder,
};
pub use health::{HealthAssessment, HealthPolicy, HealthTracker, HealthVerdict, VenueHealth};
pub use mirror::{
    Direction, DistributedReference, InventoryBand, MirrorPermission, PermittedDirection,
    RegionPosture, RegionState, SizeDiscipline, direction_gate,
};
pub use ordertype::{
    OrderTypeKind, OrderTypeSelection, PegReference, RoutedOrderType, Touch, Urgency,
    select_order_type,
};
pub use path::{
    Composition, CompositionEdge, Coordination, EdgeClass, ExecutionPath, MAX_COMPOSITION_EDGES,
    MIN_COMPOSITION_EDGES, MirrorFacts, PathAssignment, PathEndpoint, PathPolicy, RegionId, assign,
    eligible_paths,
};
pub use pathcycle::{CycleRouter, RepresentationClasses, VenueRegions};
pub use ratelimit::{MAX_TRACKED_PER_WINDOW, RateLedger, RateLimits};
pub use reprice::{
    Drift, HoldReason, PendingReplace, RepriceDecision, RepricePolicy, Repricer, ThrottleScope,
};
pub use router::{
    ExclusionReason, RouteSlice, Router, RouterSettings, RoutingDecision, RoutingRequest,
    VenueCandidate, VenueExclusion,
};
pub use venue::{FeeSchedule, FeeTier, Liquidity, VenueProfile};
