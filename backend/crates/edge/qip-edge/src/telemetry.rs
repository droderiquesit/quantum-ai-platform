//! The cell's metric seam.
//!
//! A cell knows several things no operator could see. Its policy freshness was
//! computed, formatted into a display string and journaled; its degradation
//! narrowing decided how large it may size and then evaporated; `is_halted()`
//! — the single most important boolean in the cell — reached a JSON health
//! body nothing collects as a series. A fact a process knows and never records
//! is a fact nobody can chart, alert on, or correlate against the central
//! plane that caused it.
//!
//! **The cell records into a registry it is given.** [`CellMetrics`] holds an
//! `Arc<Metrics>` handed to it by the composition root, never one it reached
//! for. That keeps the boundary rule intact in both directions: `qip-edge`
//! depends on `qip-observability`, which is a library holding an in-memory
//! `BTreeMap` behind a mutex and performs no I/O, and the decision about
//! *where* the numbers are served from stays in `qip-edge-node` where every
//! other deployment decision lives.
//!
//! **Nothing here can block or fail the hot path.** Every method returns `()`.
//! The only synchronisation is `Metrics`' own mutex, whose critical section is
//! a `BTreeMap` lookup and an integer add, and which recovers from poisoning
//! rather than panicking. There is no I/O, no allocation of unbounded size, no
//! error to propagate and nothing a caller must check — so a recording site
//! cannot become a reason an order was not sent.
//!
//! **Cardinality is bounded by construction.** `cell` and `region` are fixed
//! for the life of the process. `venue` is bounded by the cell's configured
//! venue set. `gate` is bounded by the string literals `Cell::refuse` is
//! called with, **plus the `pub const`s passed at the seams that record
//! directly** — `qip_edge::cell::GATE_LIVE_VENUE` from `Cell::send`, and
//! `qip_edge::cell::GATE_QUOTE_BUDGET` from both the withdrawal seam and
//! `Cell::spend_requote`. Each of those happens where an order or a message
//! would leave and has no `WorkReport` to push a refusal onto.
//!
//! **The invariant is not the number of sites, and this paragraph twice said
//! it was.** It read "the literals `Cell::refuse` is called with" until a
//! second seam appeared, then named `GATE_LIVE_VENUE` as "the one gate" the
//! cell records outside `Cell::refuse` — which was already false when it was
//! written, because §29.2's withdrawal seam records `GATE_QUOTE_BUDGET` the
//! same way. An enumeration goes stale silently every time a seam is added,
//! and a reader who trusts it concludes a legitimate site is a violation. So
//! the bound is stated as a property instead: **every site passes a `pub
//! const` or a source-file literal, never a runtime string.** A constant is
//! as fixed as a literal however many seams use one, and a single site
//! passing a `format!` would break the bound at any count. Count the sites
//! with `grep -n 'metrics\.refusal(' backend/crates/edge/qip-edge/src/cell.rs`
//! to know where to look, then read each one — the count tells you nothing on
//! its own. `source`, `kind` and `outcome` are
//! enums, and `capability` is
//! the three policy-fed variants of one. Nothing here is labelled by
//! instrument, strategy or order id, and that is deliberate: a series per
//! order id is a memory leak wearing a dashboard. Each `with(...)` call below
//! says what bounds its own label.

use crate::decomposition::Completion;
use crate::passive::PassiveOutcome;
use crate::quoting::{MessageKind, VenueBudgetState};
use crate::region::DarkSource;
use qip_contracts::degradation::{Capability, DegradationState, Freshness};
use qip_contracts::signal::SignalKind;
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::time::Duration;
use qip_observability::metrics::{Histogram, Labels, Metrics, names};
use std::sync::Arc;

/// Fills the venue reported and the cell booked, by venue.
///
/// Named here rather than in `qip_observability::metrics::names` because
/// that module is the observability owner's; the name follows its
/// `qip_edge_*_total` convention so it can move there unchanged.
pub const EDGE_FILLS_CONFIRMED: &str = "qip_edge_fills_confirmed_total";

/// Resting orders the cell withdrew at their time to live, by venue. Named
/// here for the same reason as [`EDGE_FILLS_CONFIRMED`].
pub const EDGE_ORDERS_EXPIRED: &str = "qip_edge_orders_expired_total";

/// Resting orders withdrawn because the touch moved past the declared drift
/// threshold and re-sent at the touch under a fresh id, by venue. Named here
/// for the same reason as [`EDGE_FILLS_CONFIRMED`]. Recorded by the node's
/// requoter rather than by the cell, which has no repricing of its own: the
/// cell's record keeps one id per intention and the venue sees the fresh one.
pub const EDGE_ORDERS_REPRICED: &str = "qip_edge_orders_repriced_total";

/// Whether this cell was given a region allocation at all: `1` or `0`. Named
/// here for the same reason as [`EDGE_FILLS_CONFIRMED`].
pub const EDGE_REGION_ALLOCATION_CONFIGURED: &str = "qip_edge_region_allocation_configured";

/// What the region allocation has left, published only by a cell that holds
/// one. Named here for the same reason as [`EDGE_FILLS_CONFIRMED`].
pub const EDGE_REGION_ALLOCATION_FREE: &str = "qip_edge_region_allocation_free";

/// The bound the region table holds after a share moved it: the centre's
/// number, capped by the operator's ceiling (ADR 0039). Named here for the
/// same reason as [`EDGE_FILLS_CONFIRMED`].
///
/// Distinct from [`EDGE_REGION_ALLOCATION_FREE`], which is what is left of
/// it once every hold and commitment is counted. The two answer different
/// questions and a cell that has spent its share to the last unit reports a
/// free of zero under a bound that has not moved — which is exactly the case
/// where one number alone cannot say whether the centre narrowed the cell or
/// the cell simply traded.
pub const EDGE_REGION_SHARE_BOUND: &str = "qip_edge_region_share_bound";

/// What became of each attempt to move the cell's region share, by outcome.
/// Named here for the same reason as [`EDGE_FILLS_CONFIRMED`].
pub const EDGE_REGION_SHARE_APPLIED: &str = "qip_edge_region_share_applied_total";

/// What an attempt to move the cell's region share came to (ADR 0039).
///
/// The `outcome` label's whole range, as a type rather than as four string
/// literals at four call sites. The bound is what the cell may commit in
/// total, and it moves for reasons an operator cannot otherwise separate: the
/// centre narrowed the cell, or a grant landed and the same manifest summed
/// higher, or a replayed payload was turned down, or the centre shipped a
/// payload that said nothing about capital at all. Charted as one gauge those
/// four are indistinguishable — a bound that did not move looks the same
/// whether nothing was offered or something was refused — and "the share did
/// not change" is the reading under which a cell starved by a stuck downlink
/// and a cell the centre deliberately narrowed are the same picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionShareOutcome {
    /// A signed payload's grant manifest was summed and the ledger re-based
    /// to it — the centre's number, newly applied.
    Applied,
    /// The manifest already applied was summed again under its own sequence,
    /// because the grants this cell holds had changed. Read beside `applied`:
    /// re-derivations without a re-base in between are the cell catching up
    /// with a plan that deployed after the payload naming it.
    Rederived,
    /// The ledger turned the share down under a sequence no newer than the
    /// one it already holds. The replay ADR 0008 exists to make harmless, and
    /// the one refusal an operator must be able to see without reading a
    /// journal: it means something is re-sending old payloads.
    RefusedLowerSequence,
    /// A payload arrived and its `capital_grants` slot was unproduced, so the
    /// table was left exactly as it was. Not a refusal and not an error — the
    /// centre said nothing about capital — but the difference between "the
    /// centre narrowed us to nothing" and "the centre has stopped telling us
    /// anything" is the difference between a plan and an outage.
    Withheld,
}

impl RegionShareOutcome {
    /// The label value. A method rather than a `Display` so that the series
    /// identity is a `&'static str` chosen here and nothing can format a
    /// fifth one into it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Rederived => "rederived",
            Self::RefusedLowerSequence => "refused_lower_sequence",
            Self::Withheld => "withheld",
        }
    }
}

/// Messages the cell's per-venue budget has left (§29.2), by venue.
///
/// Named here rather than in `qip_observability::metrics::names` for the same
/// reason as [`EDGE_FILLS_CONFIRMED`]. Written on every pass for every
/// configured venue, including the passes where the cell sent nothing and the
/// ones where it is halted: a quiet cell with a full bucket and a quiet cell
/// that has spent its rate limit are the two readings an operator must be
/// able to tell apart, and only one of them is a problem.
pub const EDGE_QUOTE_BUDGET_TOKENS: &str = "qip_edge_quote_budget_tokens";

/// Whether the message-to-trade monitor has narrowed quoting at a venue:
/// `1` or `0`, by venue (§29.2). Written on every pass, so a recovery is a
/// series falling to zero rather than one that stops being updated.
pub const EDGE_QUOTE_NARROWED: &str = "qip_edge_quote_narrowed";

/// Messages the cell sent to a venue, by venue and by what the message does
/// to the cell's exposure (§29.2). Read beside [`EDGE_QUOTE_BUDGET_TOKENS`]:
/// the rate the cell is actually producing against the budget it has left.
pub const EDGE_MESSAGES_SENT: &str = "qip_edge_messages_sent_total";

/// Resting orders withdrawn by a mass cancel because the cell is halted, by
/// venue (§29.2). Distinct from [`EDGE_ORDERS_EXPIRED`] on purpose: an order
/// that reached its time to live and an order pulled because the kill switch
/// tripped are the same action for two entirely different reasons, and a
/// single series would hide a halt inside ordinary housekeeping.
pub const EDGE_ORDERS_MASS_CANCELLED: &str = "qip_edge_orders_mass_cancelled_total";

/// Regions this cell mirrors into that it is treating as dark, by why
/// (§36.3). Named here for the same reason as [`EDGE_FILLS_CONFIRMED`].
///
/// Counted over the regions the cell's own venue map places abroad, so the
/// number is "mirrors into this many regions are suspended" rather than "a
/// file somewhere lists this many names". Both `source` arms are written on
/// every pass, including a halted one and including the pass where nothing
/// is dark, because a cell that stops publishing this reads exactly like a
/// cell with every peer answering — and "no peer has gone dark" is the
/// finding an operator is looking for during an incident in another region.
pub const EDGE_REGIONS_DARK: &str = "qip_edge_regions_dark";

/// Venues a restarted cell must still be shown an account of before it forms
/// an order (§36.3). Named here for the same reason as
/// [`EDGE_FILLS_CONFIRMED`].
///
/// Zero for a cell that is not awaiting reconciliation, which is every cell
/// that did not restart, and written on every pass for the reason
/// [`EDGE_REGIONS_DARK`] is: a node paused pending reconciliation sends
/// nothing, and a node that is merely quiet sends nothing, and those are the
/// two states this series exists to tell apart.
pub const EDGE_VENUES_AWAITING_RECONCILIATION: &str = "qip_edge_venues_awaiting_reconciliation";

/// How long each venue took from the cell's send to the venue's own confirmed
/// fill, in milliseconds, by venue (§32.1). The measurement the dispersion
/// gate is built on, published so that a refusal can be checked against the
/// history that produced it.
pub const EDGE_FILL_TIME_MILLIS: &str = "qip_edge_fill_time_millis";

/// Legs of an arbitrage cycle the cell sent, by what the venue reported each
/// of them completed (§32.1's size decomposition).
///
/// One record per leg that reached a venue, and none for a leg the cell
/// declined to send — that refusal is counted under the
/// `arbitrage_cycle_broken` gate on [`names::EDGE_REFUSALS`], and counting it
/// here as well would make one stopped cycle read as two.
///
/// `completion` is [`crate::decomposition::Completion::as_str`], four
/// source-file literals on a `Copy` enum, so the series is bounded by that
/// enum and never by anything a venue said. Read as a whole rather than one
/// arm at a time: a cell whose legs are all `unanswered` is a cell whose
/// gateway has no order-entry channel, and it decomposes nothing for a reason
/// that has nothing to do with the market; a cell whose legs are all `whole`
/// is the ordinary healthy reading. Those two are indistinguishable on any
/// series that counted only the legs that filled short.
pub const EDGE_CYCLE_LEGS: &str = "qip_edge_cycle_legs_total";

/// Arbitrage cycles by what §32.1's passive-first mechanism did with them.
///
/// `outcome` is [`crate::passive::PassiveOutcome::as_str`], six source-file
/// literals across two `Copy` enums, so the series is bounded by them and
/// never by a venue name or a cycle id.
///
/// Read as a whole rather than one arm at a time. The three declining arms —
/// `single_venue`, `unmeasured`, `no_slowest` — are the denominator, and
/// without them a cell that has never rested a leg and a cell that has never
/// run a cycle are the same empty series. `abandoned` is the arm the
/// mechanism exists to produce: a cycle whose slow leg was withdrawn without
/// filling anything never became a position at all, where the all-at-once
/// discipline would already have crossed the fast legs against it.
pub const EDGE_PASSIVE_CYCLES: &str = "qip_edge_passive_cycles_total";

/// Configured venues that have produced too few fills for their fill time to
/// be judged (§32.1).
///
/// The idle reading, and the reason this series exists at all: with every
/// venue unmeasured the dispersion gate admits every cycle on no evidence,
/// which looks exactly like a gate that is passing. One number, not per
/// venue, because the question is whether the control has anything to work
/// with.
pub const EDGE_FILL_TIME_UNMEASURED: &str = "qip_edge_fill_time_unmeasured_venues";

/// Configured venues the cell holds no settlement terms for (§56.2 rule 21).
///
/// The idle reading of the settlement gate, for the reason
/// [`EDGE_FILL_TIME_UNMEASURED`] exists: a leg that spends proceeds at a
/// venue with no terms is admitted unjudged, and every venue unstated is a
/// gate that reads as passing on a chart of refusals. One number rather
/// than per venue, because the question is whether the control has anything
/// to project against. Written every pass, including a halted one.
pub const EDGE_SETTLEMENT_UNPROJECTED: &str = "qip_edge_settlement_unprojected_venues";

/// Buckets for the netting ratio.
///
/// It is a ratio of gross intent to net order volume, so it starts at exactly
/// `1.0` — a cell whose strategies never agree or offset — and rises without
/// bound as they do. The lower buckets are tight because the interesting
/// question is whether a set that claims diversity is actually netting at all,
/// and `1.0` against `1.1` is the whole answer to it.
fn netting_ratio_buckets() -> Histogram {
    Histogram::with_bounds(vec![1.0, 1.05, 1.1, 1.25, 1.5, 2.0, 3.0, 5.0, 10.0, 50.0])
}

/// Where a cell's facts go.
///
/// The default discards into a registry nobody reads, so a `Cell` assembled
/// without telemetry — every unit test in the tree, and any caller that has
/// not been given a handle — records into somewhere harmless rather than
/// forcing an `Option` check at two dozen call sites. An `Option` would put a
/// branch on the hot path whose only purpose is to decide whether to do
/// nothing.
#[derive(Debug)]
pub struct CellMetrics {
    metrics: Arc<Metrics>,
    /// `cell` and `region`, resolved once. Cloned per recording rather than
    /// rebuilt, because the label set is the series identity and building it
    /// twice from different code would be two series for one fact.
    base: Labels,
}

impl CellMetrics {
    /// Record into `metrics`, tagging everything with this cell and region.
    ///
    /// Every metric name is described here. Describing them where the recorder
    /// is assembled is the one place guaranteed to run exactly once, and
    /// `Metrics::describe` keeps the text by name so a description registered
    /// before the first observation is not lost.
    pub fn new(metrics: Arc<Metrics>, cell: &str, region: &str) -> Self {
        let mut base = Labels::new();
        base.insert("cell".to_string(), cell.to_string());
        base.insert("region".to_string(), region.to_string());
        let recorder = Self { metrics, base };
        recorder.describe();
        recorder
    }

    /// A recorder whose numbers nobody reads.
    pub fn silent() -> Self {
        Self {
            metrics: Arc::new(Metrics::new("qip-edge")),
            base: Labels::new(),
        }
    }

    fn describe(&self) {
        let m = &self.metrics;
        m.describe(
            names::EDGE_WORK_PASSES,
            "passes of the cell's decide-and-act loop",
        );
        m.describe(
            names::EDGE_HALTED,
            "whether the cell is stopped, by which halt is in force: kill_switch, policy, polled",
        );
        m.describe(
            names::EDGE_REFUSALS,
            "gates that refused, by gate — why the cell was quiet",
        );
        m.describe(names::EDGE_SIGNALS_RAISED, "signals raised, by kind");
        m.describe(
            names::EDGE_CAPABILITY_FRESHNESS,
            "capability freshness: 0 fresh, 1 stale, 2 unavailable",
        );
        m.describe(
            names::EDGE_SIZING_MULTIPLIER,
            "the degradation table's sizing multiplier in force",
        );
        m.describe(
            names::EDGE_POLICY_SEQUENCE,
            "the sequence of the policy payload this cell has applied",
        );
        m.describe(
            names::EDGE_NETTING_RATIO,
            "gross intent over net order volume, per pass",
        );
        m.describe(
            names::EDGE_ORDERS_PLACED,
            "orders sent to a venue, by venue",
        );
        m.describe(
            EDGE_FILLS_CONFIRMED,
            "fills the venue reported and the cell booked, by venue",
        );
        m.describe(
            EDGE_ORDERS_EXPIRED,
            "resting orders withdrawn at their time to live, by venue",
        );
        m.describe(
            EDGE_ORDERS_REPRICED,
            "resting orders withdrawn for drift and re-sent at the touch, by venue",
        );
        m.describe(
            EDGE_REGION_ALLOCATION_CONFIGURED,
            "whether an operator gave this cell a region allocation to hold against",
        );
        m.describe(
            EDGE_REGION_ALLOCATION_FREE,
            "capital the cell's region allocation has left, no hold standing on it",
        );
        m.describe(
            EDGE_REGION_SHARE_BOUND,
            "the bound the cell's region table holds: its share of the region's grant, capped by \
             the operator's ceiling",
        );
        m.describe(
            EDGE_REGION_SHARE_APPLIED,
            "attempts to move the cell's region share, by outcome: applied, rederived, \
             refused_lower_sequence, withheld",
        );
        m.describe(
            names::EDGE_INTENTS_CANCELLED,
            "net intents that cancelled to zero and never reached a venue",
        );
        m.describe(
            names::EDGE_INTERNAL_CROSSES,
            "portions crossed between the platform's own strategies, by venue",
        );
        m.describe(
            EDGE_QUOTE_BUDGET_TOKENS,
            "messages the per-venue quote budget has left, by venue",
        );
        m.describe(
            EDGE_QUOTE_NARROWED,
            "whether the message-to-trade monitor has narrowed quoting at a venue",
        );
        m.describe(
            EDGE_MESSAGES_SENT,
            "messages sent to a venue, by venue and kind: placement, withdrawal",
        );
        m.describe(
            EDGE_ORDERS_MASS_CANCELLED,
            "resting orders withdrawn by a mass cancel because the cell is halted, by venue",
        );
        m.describe(
            EDGE_FILL_TIME_MILLIS,
            "milliseconds from the cell's send to the venue's confirmed fill, by venue",
        );
        m.describe(
            EDGE_CYCLE_LEGS,
            "legs of an arbitrage cycle the cell sent, by what the venue reported each \
             completed: unanswered, whole, short, unviable",
        );
        m.describe(
            EDGE_FILL_TIME_UNMEASURED,
            "configured venues with too few fills for their fill time to be judged",
        );
        m.describe(
            EDGE_SETTLEMENT_UNPROJECTED,
            "configured venues with no settlement terms, whose dependent cycle legs are \
             admitted without a settlement projection",
        );
        m.describe(
            EDGE_PASSIVE_CYCLES,
            "arbitrage cycles by what passive-first did with them: rested, completed, \
             abandoned, or sent whole under single_venue, unmeasured or no_slowest",
        );
        m.describe(
            EDGE_REGIONS_DARK,
            "regions this cell mirrors into that it is treating as dark, by source: declared, \
             unreadable",
        );
        m.describe(
            EDGE_VENUES_AWAITING_RECONCILIATION,
            "venues a restarted cell must still be shown an account of before it forms an order",
        );
        m.describe(
            names::EDGE_RECONCILIATION_BREAKS,
            "disagreements between the cell's fills and the venue's own account",
        );
    }

    /// The registry this recorder writes to, for a composition root that has
    /// to serve the same one it handed over.
    pub fn registry(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    fn with(&self, key: &str, value: &str) -> Labels {
        let mut labels = self.base.clone();
        labels.insert(key.to_string(), value.to_string());
        labels
    }

    /// One pass of `Cell::work` began.
    ///
    /// Recorded unconditionally at the top of the pass, including the halted
    /// one that returns immediately. Without it every other edge series is
    /// unreadable: a refusal count of zero means "nothing was refused" and
    /// "the cell never ran" identically, and those are the two most different
    /// states a cell has.
    pub fn work_pass(&self) {
        self.metrics
            .count(names::EDGE_WORK_PASSES, self.base.clone());
    }

    /// The halt state, by source.
    ///
    /// All three sources are written on every call, so a release shows as
    /// the series falling to zero rather than as a series that stops being
    /// updated. A gauge that goes stale at `1` and a cell that is still halted
    /// look identical on a chart.
    pub fn halt(&self, kill_switch: bool, policy: bool, polled: bool) {
        // `source` takes exactly the three literals below — one per halt
        // discipline the cell has — so this is three series per cell. The
        // third is §46.2's second wire, charted on its own so an operator
        // can see which path stopped the cell and, after an incident, which
        // one did not.
        self.metrics.gauge(
            names::EDGE_HALTED,
            self.with("source", "kill_switch"),
            f64::from(u8::from(kill_switch)),
        );
        self.metrics.gauge(
            names::EDGE_HALTED,
            self.with("source", "policy"),
            f64::from(u8::from(policy)),
        );
        self.metrics.gauge(
            names::EDGE_HALTED,
            self.with("source", "polled"),
            f64::from(u8::from(polled)),
        );
    }

    /// How many of the regions this cell mirrors into are dark, by source.
    ///
    /// Both arms are written on every call for the reason [`Self::halt`]
    /// writes all three of its own: a gauge that stops being updated and a
    /// region that came back read identically on a chart. `source` takes the
    /// two values of [`DarkSource`] and nothing else — never a region name,
    /// which arrives from outside the process and is exactly the unbounded
    /// label this module's header refuses.
    pub fn regions_dark(&self, source: Option<DarkSource>, count: usize) {
        for arm in [DarkSource::Declared, DarkSource::Unreadable] {
            let dark = if source == Some(arm) { count } else { 0 };
            self.metrics.gauge(
                EDGE_REGIONS_DARK,
                self.with("source", arm.as_str()),
                dark as f64,
            );
        }
    }

    /// How many venues a restarted cell is still waiting on.
    ///
    /// No label beyond the cell's own: the venues are named in the journal
    /// and in the refusal, and a series per venue would put the same fact on
    /// the chart twice — once as a count that can reach zero and once as a
    /// set of gauges that never can.
    pub fn awaiting_reconciliation(&self, venues: usize) {
        self.metrics.gauge(
            EDGE_VENUES_AWAITING_RECONCILIATION,
            self.base.clone(),
            venues as f64,
        );
    }

    /// A gate refused.
    ///
    /// `gate` is a string literal at every call site, or one of the
    /// `GATE_*` constants `crate::feasibility` names its rules by — `Cell::refuse`
    /// is never handed a formatted string, and the reason, which is formatted,
    /// goes to the journal and not to a label. The series count is the number
    /// of distinct literals in `cell.rs` and `feasibility.rs`, which is a
    /// property of the source and not of the market.
    pub fn refusal(&self, gate: &str) {
        self.metrics
            .count(names::EDGE_REFUSALS, self.with("gate", gate));
    }

    /// A strategy raised a signal.
    ///
    /// Keyed on the signal's kind, a four-variant enum, and deliberately not
    /// on the strategy or the instrument that raised it: both are unbounded
    /// over the life of a cell, and the question this series answers — is the
    /// cell seeing anything to act on — does not need either.
    pub fn signal(&self, kind: SignalKind) {
        self.metrics
            .count(names::EDGE_SIGNALS_RAISED, self.with("kind", kind.as_str()));
    }

    /// The degradation narrowing in force, at the instant it was derived.
    ///
    /// Freshness is a function of *now*, so it becomes known once per pass and
    /// not when a payload was applied. Recording it at the seam where the cell
    /// consults it is what makes the series say what the cell actually sized
    /// against, rather than what it would have sized against at some earlier
    /// instant.
    ///
    /// Only the capabilities a policy payload feeds are published: the causal
    /// graph, episodic memory and the belief state — the three
    /// `PolicyItem::capability` maps, and the two that set the sizing
    /// multiplier. `Ingestion` is deliberately absent because
    /// `Cell::narrowing` never observes it: book staleness is refused per book
    /// at the routing seam and counted under the `stale_book` gate, and the
    /// table's `unavailable` for it is `nothing_known()`'s default, not a
    /// measurement. `CounterfactualScoring` never ships and §6.2 gives its
    /// loss no trading impact. Publishing either would put a permanent `2` on
    /// a chart whose whole purpose is a `max`, and an operator would learn to
    /// ignore the one series that pages on a real narrowing.
    ///
    /// Each of the three is written on every pass, so a capability that goes
    /// from stale back to fresh is a series falling to zero rather than one
    /// that stopped being updated.
    pub fn narrowing(&self, state: &DegradationState) {
        for capability in [
            Capability::CausalGraph,
            Capability::EpisodicMemory,
            Capability::BeliefState,
        ] {
            let severity = match state.freshness(capability) {
                Freshness::Fresh => 0.0,
                Freshness::Stale => 1.0,
                Freshness::Unavailable => 2.0,
            };
            // `capability` is one of the three variants named above; the
            // series count is three per cell, whatever the payload carries.
            self.metrics.gauge(
                names::EDGE_CAPABILITY_FRESHNESS,
                self.with("capability", capability.as_str()),
                severity,
            );
        }
        // The multiplier is `Decimal` because it scales money. This is the
        // crossing point to `f64`, and it is a reporting one: the number is
        // exported for a human to look at and is never multiplied back into a
        // size. The size the cell actually uses stays `Decimal` throughout.
        self.metrics.gauge(
            names::EDGE_SIZING_MULTIPLIER,
            self.base.clone(),
            state.sizing_multiplier().to_f64(),
        );
    }

    /// The region allocation, per pass.
    ///
    /// Two series rather than one, because the absent case is the one an
    /// operator most needs to see. A cell with no allocation publishes
    /// `configured = 0` and no free balance: a free balance of zero would
    /// read as a cell that has spent everything, and publishing its
    /// envelopes' total would be a number nobody computed. A missing series
    /// is not enough on its own — nobody notices a series that is not there —
    /// so the boolean is written on every pass either way.
    pub fn region_allocation(&self, free: Option<Decimal>) {
        self.metrics.gauge(
            EDGE_REGION_ALLOCATION_CONFIGURED,
            self.base.clone(),
            f64::from(u8::from(free.is_some())),
        );
        if let Some(free) = free {
            // The crossing point from `Decimal` to `f64`, and a reporting one:
            // the balance the cell holds against stays `Decimal` wherever it
            // is arithmetic.
            self.metrics.gauge(
                EDGE_REGION_ALLOCATION_FREE,
                self.base.clone(),
                free.to_f64(),
            );
        }
    }

    /// The bound the region table holds, at the instant a share moved it.
    ///
    /// Recorded at the seam where the ledger accepted a share and nowhere
    /// else, so the series says what the centre's arithmetic came to rather
    /// than what the cell would report if asked. A refusal leaves the previous
    /// value standing, which is the truth: a refused share changes no bound.
    pub fn region_share_bound(&self, bound: Decimal) {
        // The bound is `Decimal` because it is money the cell may commit, and
        // it stays `Decimal` everywhere it is compared against a hold. This is
        // the crossing point to `f64` and it is a reporting one: the number
        // leaves here for a chart and never returns to the ledger.
        self.metrics
            .gauge(EDGE_REGION_SHARE_BOUND, self.base.clone(), bound.to_f64());
    }

    /// What one attempt to move the region share came to.
    ///
    /// `outcome` is [`RegionShareOutcome`], so the series count is four per
    /// cell whatever the centre publishes. The sequence, the share and the
    /// refusal's reason are journaled; none of them is a label, because a
    /// sequence rises without bound and a reason is formatted.
    pub fn region_share(&self, outcome: RegionShareOutcome) {
        self.metrics.count(
            EDGE_REGION_SHARE_APPLIED,
            self.with("outcome", outcome.as_str()),
        );
    }

    /// The sequence of the policy payload the cell has applied.
    ///
    /// The central plane knows what it published; this is what arrived and was
    /// accepted. The two being charted side by side is the only way a stuck
    /// downlink is visible as anything other than a cell that has quietly
    /// stopped changing its mind.
    pub fn policy_applied(&self, sequence: u64) {
        // A sequence is a counter's worth of range in a gauge on purpose: it
        // is a position, not a rate, and `increase()` over it would be
        // meaningless. Above 2^53 the `f64` stops being exact, which is some
        // hundreds of thousands of years of policy at any rate a signing
        // central plane can produce.
        self.metrics.gauge(
            names::EDGE_POLICY_SEQUENCE,
            self.base.clone(),
            sequence as f64,
        );
    }

    /// Gross intent over net order volume for one pass (§27).
    pub fn netting_ratio(&self, ratio: f64) {
        self.metrics.observe_with(
            names::EDGE_NETTING_RATIO,
            self.base.clone(),
            ratio,
            netting_ratio_buckets,
        );
    }

    /// An order reached a venue.
    ///
    /// `venue` is one of `CellConfig::venues` — `Cell::venue_for` selects
    /// from that list and nothing else — so the series count is the size of a
    /// list fixed at deployment. The order id and the instrument are not
    /// labels: an order id is a new series per order, which is a registry
    /// that grows without bound for as long as the cell trades.
    pub fn order_placed(&self, venue: &VenueId) {
        self.metrics.count(
            names::EDGE_ORDERS_PLACED,
            self.with("venue", venue.as_str()),
        );
    }

    /// The venue reported a fill and the cell booked it. Bounded on `venue`
    /// exactly as [`Self::order_placed`] is: a fill is confirmed only
    /// against an order the cell sent, whose venue came from the configured
    /// list. Read beside the orders series: orders placed and fills confirmed
    /// diverging is a venue where the cell's orders rest.
    pub fn fill_confirmed(&self, venue: &VenueId) {
        self.metrics
            .count(EDGE_FILLS_CONFIRMED, self.with("venue", venue.as_str()));
    }

    /// A resting order reached its time to live and the venue confirmed the
    /// withdrawal. Bounded on `venue` as [`Self::order_placed`] is. Read
    /// beside fills confirmed: a venue where orders expire more than they
    /// fill is a venue where resting at the mid is the wrong policy.
    pub fn order_expired(&self, venue: &VenueId) {
        self.metrics
            .count(EDGE_ORDERS_EXPIRED, self.with("venue", venue.as_str()));
    }

    /// A resting order was withdrawn for drift and its remainder re-sent at
    /// the touch — one cancel acknowledged, then one new order, never two
    /// live. Bounded on `venue` as [`Self::order_placed`] is. Read beside
    /// orders placed: a venue where requotes approach placements is a market
    /// the requote budget is the only thing stopping the cell from chasing.
    pub fn order_repriced(&self, venue: &VenueId) {
        self.metrics
            .count(EDGE_ORDERS_REPRICED, self.with("venue", venue.as_str()));
    }

    /// A net that cancelled to zero. An outcome, not an absence — which is
    /// exactly why it is counted rather than inferred from an order that did
    /// not appear.
    pub fn intent_cancelled(&self) {
        self.metrics
            .count(names::EDGE_INTENTS_CANCELLED, self.base.clone());
    }

    /// A cross was booked between two of the platform's own strategies.
    ///
    /// Keyed on the venue whose mid priced it, bounded exactly as
    /// [`Self::order_placed`] is: the venue came from the net intent, which
    /// took it from the configured list. The strategies on each side are in
    /// the journal, not on a label.
    pub fn internal_cross(&self, venue: &VenueId) {
        self.metrics.count(
            names::EDGE_INTERNAL_CROSSES,
            self.with("venue", venue.as_str()),
        );
    }

    /// Every venue's quote budget, as the pass found it (§29.2).
    ///
    /// Both series are written for every configured venue on every pass,
    /// including the halted ones, so the idle state says so rather than being
    /// a gap. `venue` is bounded by `CellConfig::venues` — the budget has one
    /// bucket per configured venue and an admission cannot mint another — and
    /// nothing here is labelled by instrument or order.
    pub fn quote_budget(&self, states: &[VenueBudgetState]) {
        for state in states {
            self.metrics.gauge(
                EDGE_QUOTE_BUDGET_TOKENS,
                self.with("venue", &state.venue),
                f64::from(state.tokens),
            );
            self.metrics.gauge(
                EDGE_QUOTE_NARROWED,
                self.with("venue", &state.venue),
                f64::from(u8::from(state.narrowed)),
            );
        }
    }

    /// One message the budget admitted and the cell sent (§29.2).
    ///
    /// `kind` is [`MessageKind`], a two-variant enum, so this is two series
    /// per venue. Counted where the budget was spent rather than inferred
    /// from orders placed: a cancel is a message the venue's rate limit
    /// counts and no order series records.
    pub fn message_sent(&self, venue: &VenueId, kind: MessageKind) {
        let mut labels = self.with("venue", venue.as_str());
        labels.insert("kind".to_string(), kind.as_str().to_string());
        self.metrics.count(EDGE_MESSAGES_SENT, labels);
    }

    /// A resting order withdrawn because the cell is halted (§29.2).
    ///
    /// Bounded on `venue` exactly as [`Self::order_placed`] is. Read beside
    /// `qip_edge_halted`: the halt says the cell stopped, this says what it
    /// pulled back on the way.
    pub fn order_mass_cancelled(&self, venue: &VenueId) {
        self.metrics.count(
            EDGE_ORDERS_MASS_CANCELLED,
            self.with("venue", venue.as_str()),
        );
    }

    /// How long one order took from send to confirmed fill (§32.1).
    ///
    /// The crossing point from the platform's `Duration` to the `f64` a
    /// histogram holds, and a reporting one: the dispersion gate compares
    /// `Duration`s throughout and never reads this number back.
    pub fn fill_time(&self, venue: &VenueId, taken: Duration) {
        // `as_nanos` is an `i64` of nanoseconds; the histogram is in
        // milliseconds, and the division is done here rather than at the call
        // site so every observation of this series is in the same unit.
        #[allow(clippy::cast_precision_loss)]
        let millis = taken.as_nanos() as f64 / 1_000_000.0;
        self.metrics.observe_latency_ms(
            EDGE_FILL_TIME_MILLIS,
            self.with("venue", venue.as_str()),
            millis,
        );
    }

    /// How many configured venues the dispersion gate cannot judge (§32.1).
    ///
    /// Written every pass, including the idle ones, because the whole point
    /// of the number is the state in which the gate admits everything.
    pub fn fill_time_unmeasured(&self, venues: usize) {
        // A count of the configured venue list, which is fixed at deployment
        // and small; the cast cannot lose a venue anybody has.
        #[allow(clippy::cast_precision_loss)]
        let venues = venues as f64;
        self.metrics
            .gauge(EDGE_FILL_TIME_UNMEASURED, self.base.clone(), venues);
    }

    /// How many configured venues the settlement gate cannot project
    /// against (§56.2 rule 21).
    ///
    /// Written every pass, including the idle ones, for the reason
    /// [`Self::fill_time_unmeasured`] is: the number matters most in the
    /// state where the gate admits everything.
    pub fn settlement_unprojected(&self, venues: usize) {
        // A count of the configured venue list, fixed at deployment and
        // small; the cast cannot lose a venue anybody has.
        #[allow(clippy::cast_precision_loss)]
        let venues = venues as f64;
        self.metrics
            .gauge(EDGE_SETTLEMENT_UNPROJECTED, self.base.clone(), venues);
    }

    /// One leg of an arbitrage cycle, counted by what the leg before it
    /// completed (§32.1).
    ///
    /// Recorded where the fact becomes known: inside `Cell::place_cycle`, at
    /// the moment the size of the next leg is decided, which is the only
    /// place the venue's answer about the previous leg and the decision taken
    /// on it exist together.
    pub fn cycle_leg(&self, completion: Completion) {
        self.metrics.count(
            EDGE_CYCLE_LEGS,
            self.with("completion", completion.as_str()),
        );
    }

    /// What §32.1's passive-first mechanism did with one arbitrage cycle.
    ///
    /// Recorded where each fact becomes known and nowhere else: `whole` and
    /// `rested` inside `Cell::place_cycle` at the moment the choice is taken,
    /// `completed` and `abandoned` on the later pass where the resting leg's
    /// own venue has answered. Four separate instants, which is why this is
    /// one counter with an outcome rather than a gauge of a state — a cycle
    /// that rested and then completed is two facts and not one that changed.
    pub fn passive_cycle(&self, outcome: PassiveOutcome) {
        self.metrics
            .count(EDGE_PASSIVE_CYCLES, self.with("outcome", outcome.as_str()));
    }

    pub fn reconciliation_break(&self) {
        self.metrics
            .count(names::EDGE_RECONCILIATION_BREAKS, self.base.clone());
    }
}

impl Default for CellMetrics {
    fn default() -> Self {
        Self::silent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_observability::metrics::labels;

    fn recorder() -> CellMetrics {
        CellMetrics::new(Arc::new(Metrics::new("qip-edge")), "cell-a", "eu-west")
    }

    #[test]
    fn a_released_halt_reads_as_zero_rather_than_as_a_series_that_stopped() {
        // The failure this prevents: writing the gauge only while halted. The
        // series would sit at 1 forever after the first halt and an operator
        // would page on a cell that resumed hours ago.
        let recorder = recorder();
        recorder.halt(true, false, false);
        let halted = recorder.registry().snapshot();
        assert_eq!(
            halted.gauge(
                names::EDGE_HALTED,
                &labels([
                    ("cell", "cell-a"),
                    ("region", "eu-west"),
                    ("source", "kill_switch")
                ])
            ),
            Some(1.0),
            "the premise failed: the kill-switch gauge was never set to 1"
        );

        recorder.halt(false, false, false);
        let released = recorder.registry().snapshot();
        assert_eq!(
            released.gauge(
                names::EDGE_HALTED,
                &labels([
                    ("cell", "cell-a"),
                    ("region", "eu-west"),
                    ("source", "kill_switch")
                ])
            ),
            Some(0.0),
            "a released kill switch left the gauge asserting the cell is still halted"
        );
    }

    #[test]
    fn only_the_capabilities_a_payload_feeds_are_published_and_each_reports_even_when_absent() {
        // Two properties, and both matter. The three policy-fed capabilities
        // must appear even when nothing was observed: absence is the worst
        // case in the §6.2 table, and a missing series reads as good news on
        // every dashboard ever built. And the two the cell never measures
        // must *not* appear: `nothing_known()` reports ingestion as
        // unavailable by default, not by observation, and a permanent `2` on
        // a chart whose purpose is a `max` teaches an operator to ignore it.
        let recorder = recorder();
        recorder.narrowing(&DegradationState::nothing_known());
        let snapshot = recorder.registry().snapshot();
        for capability in [
            Capability::CausalGraph,
            Capability::EpisodicMemory,
            Capability::BeliefState,
        ] {
            assert_eq!(
                snapshot.gauge(
                    names::EDGE_CAPABILITY_FRESHNESS,
                    &labels([
                        ("capability", capability.as_str()),
                        ("cell", "cell-a"),
                        ("region", "eu-west")
                    ])
                ),
                Some(2.0),
                "{} did not report as unavailable with nothing known",
                capability.as_str()
            );
        }
        for capability in [Capability::Ingestion, Capability::CounterfactualScoring] {
            assert_eq!(
                snapshot.gauge(
                    names::EDGE_CAPABILITY_FRESHNESS,
                    &labels([
                        ("capability", capability.as_str()),
                        ("cell", "cell-a"),
                        ("region", "eu-west")
                    ])
                ),
                None,
                "{} was published although the cell never measures it",
                capability.as_str()
            );
        }
    }

    #[test]
    fn the_netting_ratio_lands_in_a_bucket_that_separates_no_netting_from_some() {
        // A ratio of 1.0 is a strategy set that never offsets and 1.2 is one
        // that does. Buckets that put both in the same bin would answer §27's
        // question with "yes" whatever the truth was.
        let recorder = recorder();
        recorder.netting_ratio(1.0);
        recorder.netting_ratio(1.2);
        let snapshot = recorder.registry().snapshot();
        let histogram = snapshot
            .histogram(
                names::EDGE_NETTING_RATIO,
                &labels([("cell", "cell-a"), ("region", "eu-west")]),
            )
            .expect("the netting ratio histogram was not recorded at all");
        assert_eq!(histogram.count, 2, "both observations should be counted");
        assert_eq!(
            histogram.counts[0], 1,
            "1.0 did not land in the first bucket, so no-netting is indistinguishable from netting"
        );
    }
}
