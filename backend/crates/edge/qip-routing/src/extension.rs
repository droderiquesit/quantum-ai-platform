//! Blueprint §33.1's path extensions: the additional check each of §30.2's
//! eight execution paths carries, on top of whatever gate every order already
//! passes.
//!
//! §33.1's own words about the shape: *"Every extension is additive and
//! deterministic. The gate remains a pure function over cached policy and
//! local state, which keeps it testable against fixtures and fast enough for
//! the microsecond budget. Every verdict, including silence, is logged and
//! counterfactually scored."* Nothing here reads a clock it was not handed,
//! nothing here allocates a venue, and [`check`] is a function of its
//! arguments alone.
//!
//! # Additive means additive — this can only ever subtract
//!
//! An extension is a **check**, never a routing authority. [`check`] returns
//! either a verdict that the path's own condition held, or a refusal. There
//! is no return value by which it can widen anything: it cannot name a venue,
//! cannot choose a path, cannot raise a size and cannot make a cycle eligible
//! that [`crate::path::assign`] did not already assign. A caller that ignores
//! its refusal has not gained a permission, it has skipped a check.
//!
//! # A missing fact is a refusal, never a pass
//!
//! Each of §33.1's six rows needs something the platform must actually know.
//! When the fact is absent, [`check`] **refuses**, and the message names what
//! is missing. The alternative — treating an absent fact as "nothing to
//! check" — is precisely the control that reads as protection and cannot
//! fire, and it is the reason [`crate::path::ExecutionPath`] is not
//! `#[non_exhaustive]`: the `match` in [`check`] names all eight arms and a
//! ninth path is a compile error rather than a silent default.
//!
//! # Paths 1 and 2 have no row, and that is stated rather than defaulted
//!
//! §33.1's table starts at path 3. Paths 1 and 2 execute inside one region
//! on a millisecond budget and the section names no extension for them, so
//! their arms return [`ExtensionVerdict::no_row`] — a verdict that says the
//! blueprint has no additional check here, which is a different fact from
//! "the check passed" and is reported as one.
//!
//! # What is reachable, and from where
//!
//! Every arm is reachable from a direct call, and this crate's tests reach
//! all eight. From a `qip_edge::Cell` the position is narrower and is stated
//! here so a reader does not take the module for eight delivered rows.
//!
//! **From a cell, the arms that can fire are 1, 2, 3 and 4.** A cell supplies
//! no resting-order state and no firm-quote window, so §30.2 never finds
//! paths 5 or 6 eligible there — `qip_edge`'s `Cell::mirror_facts_for` states
//! both as a `false` and a `None` it can honestly hold, and a cycle whose
//! only possible path was one of the two is refused whole by the router. And
//! `ArbitrageDesk::new` refuses a graph holding a synthetic edge, so paths 7
//! and 8 are unreachable one layer earlier still.
//!
//! **Path 4 is the correction, and the sentence it replaces is why this
//! paragraph is worth keeping accurate.** This read "a cell supplies no hedge
//! depth … so §30.2 never finds paths 4, 5 or 6 eligible there" and "the arms
//! that can fire are 1, 2 and 3", which stopped being true when
//! `qip_edge`'s `Cell::local_hedges_for` landed: a mirrored cell holds books
//! for every venue its operator configured, which is more than the cycle's
//! own instruments, so it can state a local hedge's depth. The hedge is read
//! once and handed to both the router and [`check`], so a cycle cannot be
//! assigned path 4 on one snapshot and gated on another. A reader who
//! believed the old sentence would have found [`HedgeExtension`] guarding a
//! path nothing could reach and been right to delete it — which is how a
//! working control is removed by someone tidying up, and the reason the
//! claim is stated as a runnable check rather than left as prose:
//!
//! ```text
//! grep -n 'ExecutionPath::HedgedBridging' backend/crates/edge/qip-edge/tests/cross_region.rs
//! ```
//!
//! Those assertions sit inside tests that drive a real `Cell::work` pass, so
//! they fail if the cell stops reaching the arm. Nothing here is deployed:
//! `qip-edge-node` runs `Cell::work` only under the simulated feed, and no
//! execution node exists.

use crate::mirror::{
    Direction, DistributedReference, InventoryBand, MirrorPermission, SizeDiscipline,
    direction_gate,
};
use crate::path::ExecutionPath;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::Serialize;

/// Path 3's facts — §33.1: *"Direction permitted by inventory band.
/// Reference inside TTL. Both, every time."*
///
/// Both conditions live in [`crate::mirror`]; this is the bundle the caller
/// hands over, and `intended` is the direction the caller's own cycle would
/// take at this region. The gate never invents one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MirrorExtension {
    band: InventoryBand,
    held: Decimal,
    reference: DistributedReference,
    local_price: Decimal,
    intended: Direction,
}

impl MirrorExtension {
    pub const fn new(
        band: InventoryBand,
        held: Decimal,
        reference: DistributedReference,
        local_price: Decimal,
        intended: Direction,
    ) -> Self {
        Self {
            band,
            held,
            reference,
            local_price,
            intended,
        }
    }
}

/// Path 4's facts — §33.1: *"Hedge instrument available at depth, now,
/// before the first leg."*
///
/// All three words are conditions. `depth` is what the hedge book can absorb
/// and `required` is what the cycle's first leg will need; `now` is the
/// caller's assertion that the depth was read this pass rather than
/// remembered, because a hedge that was there a minute ago is not a hedge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HedgeExtension {
    depth: Decimal,
    required: Decimal,
    read_this_pass: bool,
}

impl HedgeExtension {
    /// Refuse a requirement of nothing: a cycle that needs no hedge is not a
    /// cycle path 4 covers, and a zero requirement would make every depth
    /// sufficient.
    pub fn new(depth: Decimal, required: Decimal, read_this_pass: bool) -> Result<Self> {
        if !required.is_positive() {
            return Err(Error::invalid(format!(
                "a hedge requirement of {required} makes any depth sufficient; supply the size \
                 the first leg will need hedged"
            )));
        }
        if depth.is_negative() {
            return Err(Error::invalid(format!(
                "a hedge depth of {depth} is not a depth; supply zero when the hedge book is \
                 empty rather than a negative size"
            )));
        }
        Ok(Self {
            depth,
            required,
            read_this_pass,
        })
    }
}

/// Path 5's facts — §33.1: *"Resting order live. Adverse-selection premium
/// covered by the spread."*
///
/// The premium is what resting costs when the market runs through the order;
/// the spread is what resting earns. `Decimal` for both because the
/// comparison is money against money and it decides whether an order rests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorExtension {
    resting_live: bool,
    spread: Decimal,
    adverse_selection_premium: Decimal,
}

impl AnchorExtension {
    pub fn new(
        resting_live: bool,
        spread: Decimal,
        adverse_selection_premium: Decimal,
    ) -> Result<Self> {
        if spread.is_negative() {
            return Err(Error::invalid(format!(
                "a spread of {spread} is crossed, not a spread; supply the distance between the \
                 two sides of the book the order would rest in"
            )));
        }
        if adverse_selection_premium.is_negative() {
            return Err(Error::invalid(format!(
                "an adverse-selection premium of {adverse_selection_premium} is a payment for \
                 resting rather than a cost of it; supply zero when the estimate is that \
                 resting costs nothing"
            )));
        }
        Ok(Self {
            resting_live,
            spread,
            adverse_selection_premium,
        })
    }
}

/// Path 6's facts — §33.1: *"Window exceeds round trip plus execution plus
/// margin. Honour rate above threshold."*
///
/// The honour rate is a count of quotes honoured over quotes seen, and it is
/// held as two integers rather than a ratio on purpose. A ratio would be the
/// one statistic in this module, and comparing it against a threshold near
/// the boundary would make the check fire or not depending on a float's last
/// bit; two counts and a percentage compare exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmQuoteExtension {
    window: Duration,
    round_trip: Duration,
    execution: Duration,
    margin: Duration,
    honoured: u64,
    quoted: u64,
    honour_threshold_percent: u32,
}

impl FirmQuoteExtension {
    /// Refuse an honour rate that is not a measurement.
    ///
    /// `quoted == 0` is a venue nobody has observed quoting, and a rate
    /// computed from it would be either a division by zero or an assumed
    /// hundred per cent. §33.1 gates path 6 on a counterparty's record, and
    /// a venue with no record has not earned one.
    pub fn new(
        window: Duration,
        round_trip: Duration,
        execution: Duration,
        margin: Duration,
        honoured: u64,
        quoted: u64,
        honour_threshold_percent: u32,
    ) -> Result<Self> {
        if quoted == 0 {
            return Err(Error::invalid(
                "no quote from this venue has been observed, so it has no honour rate; path 6 \
                 rests on a counterparty's record and a venue without one has not earned it — \
                 observe quotes before routing a firm-quote bridge through it",
            ));
        }
        if honoured > quoted {
            return Err(Error::invalid(format!(
                "{honoured} quotes were honoured out of {quoted} seen, which is more honoured \
                 than offered; supply the two counts from the same window"
            )));
        }
        if honour_threshold_percent > 100 {
            return Err(Error::invalid(format!(
                "an honour threshold of {honour_threshold_percent} per cent can never be \
                 exceeded, so path 6 would be refused whatever the venue does; supply a \
                 threshold between zero and a hundred"
            )));
        }
        // The article travels with the name. "a execution" reads as a typo in
        // an operator's terminal and makes the message look machine-made
        // rather than written for them.
        for (name, value) in [
            ("a firm-quote window", window),
            ("a round trip", round_trip),
            ("an execution time", execution),
        ] {
            if value.as_nanos() <= 0 {
                return Err(Error::invalid(format!(
                    "{name} of {} ns is not a measurement; every term of §33.1's budget is \
                     measured, and a zero term makes the sum smaller than it is",
                    value.as_nanos()
                )));
            }
        }
        if margin.as_nanos() < 0 {
            return Err(Error::invalid(format!(
                "a safety margin of {} ns is negative, which spends time the budget does not \
                 have; supply zero to run with no margin and say so deliberately",
                margin.as_nanos()
            )));
        }
        Ok(Self {
            window,
            round_trip,
            execution,
            margin,
            honoured,
            quoted,
            honour_threshold_percent,
        })
    }

    /// Round trip plus execution plus margin, or a refusal when the three
    /// overflow a nanosecond count.
    fn budget(&self) -> Result<Duration> {
        let sum = self
            .round_trip
            .as_nanos()
            .checked_add(self.execution.as_nanos())
            .and_then(|partial| partial.checked_add(self.margin.as_nanos()))
            .ok_or_else(|| {
                Error::numeric(
                    "the round trip, the execution time and the margin do not sum to a \
                     nanosecond count; supply measured durations rather than sentinels",
                )
            })?;
        Ok(Duration::from_nanos(sum))
    }
}

/// Path 7's facts — §33.1: *"Expected carry exceeds financing plus
/// capital-occupancy cost to convergence."*
///
/// Three money figures to convergence, not annualised rates: a rate would
/// need a horizon to be comparable and the horizon is what "to convergence"
/// already fixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasisExtension {
    expected_carry: Decimal,
    financing: Decimal,
    capital_occupancy: Decimal,
}

impl BasisExtension {
    pub fn new(
        expected_carry: Decimal,
        financing: Decimal,
        capital_occupancy: Decimal,
    ) -> Result<Self> {
        for (name, value) in [
            ("financing", financing),
            ("capital-occupancy", capital_occupancy),
        ] {
            if value.is_negative() {
                return Err(Error::invalid(format!(
                    "a {name} cost of {value} is a payment received rather than a cost; supply \
                     zero when the cost is nothing, because a negative cost here would let a \
                     carry that does not cover financing pass the check"
                )));
            }
        }
        Ok(Self {
            expected_carry,
            financing,
            capital_occupancy,
        })
    }
}

/// Path 8's facts — §33.1: *"Net delta, gamma, vega inside limits.
/// European-style or explicit assignment budget."*
///
/// The Greeks are `Decimal` rather than `f64`, which is the crossing point
/// this module states out loud: they are conventionally floating-point
/// statistics, and here each is compared directly against a limit that stops
/// a structure being executed. A control whose firing depends on the last
/// bit of a float is a control nobody can reproduce from the log, so the
/// comparison is exact and the caller rounds on the way in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayoffExtension {
    net_delta: Decimal,
    net_gamma: Decimal,
    net_vega: Decimal,
    delta_limit: Decimal,
    gamma_limit: Decimal,
    vega_limit: Decimal,
    european_style: bool,
    assignment_budget: Option<Decimal>,
}

impl PayoffExtension {
    /// Refuse a limit that is not a limit, and an assignment budget of
    /// nothing.
    ///
    /// A negative limit could never be satisfied and would refuse every
    /// structure; a budget of zero is an American-style structure with no
    /// allowance for early assignment, presented as though it had one.
    pub fn new(
        net_delta: Decimal,
        net_gamma: Decimal,
        net_vega: Decimal,
        delta_limit: Decimal,
        gamma_limit: Decimal,
        vega_limit: Decimal,
        european_style: bool,
        assignment_budget: Option<Decimal>,
    ) -> Result<Self> {
        for (name, value) in [
            ("delta", delta_limit),
            ("gamma", gamma_limit),
            ("vega", vega_limit),
        ] {
            if value.is_negative() {
                return Err(Error::invalid(format!(
                    "a net {name} limit of {value} can never be satisfied, so path 8 would be \
                     refused whatever the structure is; supply the absolute exposure the desk \
                     will carry"
                )));
            }
        }
        if let Some(budget) = assignment_budget
            && !budget.is_positive()
        {
            return Err(Error::invalid(format!(
                "an assignment budget of {budget} is not a budget; pass None for a structure \
                 with no allowance for early assignment rather than a budget of nothing, \
                 because §33.1 reads a stated budget as the alternative to European style"
            )));
        }
        Ok(Self {
            net_delta,
            net_gamma,
            net_vega,
            delta_limit,
            gamma_limit,
            vega_limit,
            european_style,
            assignment_budget,
        })
    }
}

/// Everything §33.1 might need, with each row's facts present or absent.
///
/// Absent is the default and absent is a refusal for every path that has a
/// row. Built by the caller that holds the facts, once per cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathExtensions {
    mirror: Option<MirrorExtension>,
    hedge: Option<HedgeExtension>,
    anchor: Option<AnchorExtension>,
    firm_quote: Option<FirmQuoteExtension>,
    basis: Option<BasisExtension>,
    payoff: Option<PayoffExtension>,
}

impl PathExtensions {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn with_mirror(mut self, facts: MirrorExtension) -> Self {
        self.mirror = Some(facts);
        self
    }

    pub const fn with_hedge(mut self, facts: HedgeExtension) -> Self {
        self.hedge = Some(facts);
        self
    }

    pub const fn with_anchor(mut self, facts: AnchorExtension) -> Self {
        self.anchor = Some(facts);
        self
    }

    pub const fn with_firm_quote(mut self, facts: FirmQuoteExtension) -> Self {
        self.firm_quote = Some(facts);
        self
    }

    pub const fn with_basis(mut self, facts: BasisExtension) -> Self {
        self.basis = Some(facts);
        self
    }

    pub const fn with_payoff(mut self, facts: PayoffExtension) -> Self {
        self.payoff = Some(facts);
        self
    }
}

/// What §33.1's extension for one path found.
///
/// Carries the named conditions that held, so the log says which checks
/// actually ran rather than only that the cycle survived. Private fields and
/// no `Deserialize`: a decoder would be a second way to claim a check
/// happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExtensionVerdict {
    path: ExecutionPath,
    /// `false` for paths 1 and 2, which §33.1's table has no row for.
    has_row: bool,
    satisfied: Vec<&'static str>,
    mirror: Option<MirrorPermission>,
}

impl ExtensionVerdict {
    /// The verdict for a path §33.1 names no additional check for.
    ///
    /// Distinct from a verdict with no conditions satisfied, because the two
    /// mean opposite things: this says the blueprint asks for nothing, and
    /// that would say the platform checked nothing.
    fn no_row(path: ExecutionPath) -> Self {
        Self {
            path,
            has_row: false,
            satisfied: Vec::new(),
            mirror: None,
        }
    }

    fn with_checks(path: ExecutionPath, satisfied: Vec<&'static str>) -> Self {
        Self {
            path,
            has_row: true,
            satisfied,
            mirror: None,
        }
    }

    pub const fn path(&self) -> ExecutionPath {
        self.path
    }

    /// Whether §33.1's table has a row for this path at all.
    pub const fn has_row(&self) -> bool {
        self.has_row
    }

    /// The conditions that held, in the order §33.1 states them.
    pub fn satisfied(&self) -> &[&'static str] {
        &self.satisfied
    }

    /// Path 3's permission, when this verdict is path 3's. The one place a
    /// caller can read which direction §31.1's table permitted and under
    /// what size discipline.
    pub const fn mirror(&self) -> Option<&MirrorPermission> {
        self.mirror.as_ref()
    }

    /// The sentence a reader gets beside §33.1's table.
    pub fn rationale(&self) -> String {
        if !self.has_row {
            return format!(
                "§33.1 names no additional check for path {} ({}); the cycle carries only the \
                 checks every order carries",
                self.path.number(),
                self.path.as_str()
            );
        }
        format!(
            "path {} ({}) satisfies §33.1's extension: {}",
            self.path.number(),
            self.path.as_str(),
            self.satisfied.join("; ")
        )
    }
}

/// The name of every §33.1 condition, so a refusal and a verdict use the
/// same words and a reader can match them against the section.
const CHECK_REFERENCE_FRESH: &str = "reference inside its window";
const CHECK_DIRECTION_PERMITTED: &str = "direction permitted by the inventory band";
const CHECK_HEDGE_AT_DEPTH: &str = "hedge instrument available at depth before the first leg";
const CHECK_RESTING_LIVE: &str = "resting order live";
const CHECK_PREMIUM_COVERED: &str = "adverse-selection premium covered by the spread";
const CHECK_WINDOW_EXCEEDS_BUDGET: &str =
    "firm-quote window exceeds round trip plus execution plus margin";
const CHECK_HONOUR_RATE: &str = "honour rate above threshold";
const CHECK_CARRY_COVERS_COST: &str =
    "expected carry exceeds financing plus capital-occupancy cost to convergence";
const CHECK_GREEKS_INSIDE_LIMITS: &str = "net delta, gamma and vega inside limits";
const CHECK_EXERCISE_STYLE: &str = "European-style, or an explicit assignment budget";

/// §33.1's additional check for one assigned path.
///
/// `now` is a parameter for the reason everything else in this crate takes
/// one: a gate that read a clock could not be replayed, and §33.1 requires
/// the gate to be a pure function.
///
/// The `match` names all eight paths. That is the only way a ninth path can
/// be guaranteed to have been considered, and it is why
/// [`ExecutionPath`] is not `#[non_exhaustive]`.
pub fn check(
    path: ExecutionPath,
    extensions: &PathExtensions,
    now: Timestamp,
) -> Result<ExtensionVerdict> {
    match path {
        // §33.1's table starts at path 3. Stated, not defaulted.
        ExecutionPath::IntraVenue | ExecutionPath::CrossVenue => Ok(ExtensionVerdict::no_row(path)),

        ExecutionPath::MirroredInventory => {
            let facts = extensions.mirror.ok_or_else(|| {
                Error::invalid(
                    "path 3 was assigned and no mirror facts were supplied; §33.1 requires the \
                     direction to be permitted by the inventory band and the reference to be \
                     inside its window, both every time, so supply this region's holding, its \
                     band and the distributed reference — or do not take the cycle",
                )
            })?;
            let permission = direction_gate(
                &facts.band,
                facts.held,
                &facts.reference,
                facts.local_price,
                facts.intended,
                now,
            )?;
            if permission.posture().size() == SizeDiscipline::Reduced {
                return Err(Error::denied(format!(
                    "§31.1's at-target row permits either direction at reduced size, and this \
                     cycle was priced at the size the scan found it at; nothing in this gate \
                     can make a scanned cycle smaller, so it is refused rather than taken at \
                     full size — {}",
                    permission.rationale()
                )));
            }
            Ok(ExtensionVerdict {
                path,
                has_row: true,
                satisfied: vec![CHECK_REFERENCE_FRESH, CHECK_DIRECTION_PERMITTED],
                mirror: Some(permission),
            })
        }

        ExecutionPath::HedgedBridging => {
            let facts = extensions.hedge.ok_or_else(|| {
                Error::invalid(
                    "path 4 was assigned and no hedge facts were supplied; §33.1 requires the \
                     hedge instrument to be available at depth, now, before the first leg, so \
                     supply the depth read this pass and the size the first leg needs",
                )
            })?;
            if !facts.read_this_pass {
                return Err(Error::denied(
                    "the hedge depth was not read this pass; §33.1 says \"now, before the first \
                     leg\", and a hedge that was there a minute ago is a hedge the first leg \
                     may find gone — re-read the hedge book or do not bridge",
                ));
            }
            if facts.depth < facts.required {
                return Err(Error::denied(format!(
                    "the hedge book holds {} against the {} the first leg needs; path 4 \
                     executes locally and hedges locally, and a partial hedge leaves the \
                     unhedged remainder open across a region boundary — size the cycle to the \
                     hedge or take another path",
                    facts.depth, facts.required
                )));
            }
            Ok(ExtensionVerdict::with_checks(
                path,
                vec![CHECK_HEDGE_AT_DEPTH],
            ))
        }

        ExecutionPath::PassiveAnchoring => {
            let facts = extensions.anchor.ok_or_else(|| {
                Error::invalid(
                    "path 5 was assigned and no anchor facts were supplied; §33.1 requires the \
                     resting order to be live and the adverse-selection premium to be covered \
                     by the spread, so supply the resting state and the two figures",
                )
            })?;
            if !facts.resting_live {
                return Err(Error::denied(
                    "path 5 rests the remote leg and reprices it locally, and no resting order \
                     is live; the local leg would fire against a remote leg that does not \
                     exist — place the resting order first, or take a path that does not rest \
                     one",
                ));
            }
            if facts.spread < facts.adverse_selection_premium {
                return Err(Error::denied(format!(
                    "resting earns a spread of {} and is estimated to cost {} in adverse \
                     selection; an anchor that pays more to be run through than it earns to \
                     rest is a loss the cycle's edge has to cover twice",
                    facts.spread, facts.adverse_selection_premium
                )));
            }
            Ok(ExtensionVerdict::with_checks(
                path,
                vec![CHECK_RESTING_LIVE, CHECK_PREMIUM_COVERED],
            ))
        }

        ExecutionPath::FirmQuoteBridging => {
            let facts = extensions.firm_quote.ok_or_else(|| {
                Error::invalid(
                    "path 6 was assigned and no firm-quote facts were supplied; §33.1 requires \
                     the window to exceed round trip plus execution plus margin and the \
                     venue's honour rate to be above threshold, so supply the four durations \
                     and the two counts",
                )
            })?;
            let budget = facts.budget()?;
            if facts.window.as_nanos() <= budget.as_nanos() {
                return Err(Error::denied(format!(
                    "the firm-quote window is {} ms and the round trip, execution and margin \
                     sum to {} ms; an order that arrives as the quote dies is filled at \
                     whatever the venue has moved to, which is the one thing path 6 exists to \
                     avoid",
                    facts.window.as_millis(),
                    budget.as_millis()
                )));
            }
            // Integer arithmetic on both sides, so the comparison is exact
            // at the threshold. "Above threshold" is strict: a venue that
            // honours exactly the threshold has not exceeded it.
            let honoured_scaled = facts.honoured.checked_mul(100).ok_or_else(|| {
                Error::numeric(format!(
                    "an honoured count of {} does not scale to a percentage; supply counts from \
                     a bounded window",
                    facts.honoured
                ))
            })?;
            let threshold_scaled = facts
                .quoted
                .checked_mul(u64::from(facts.honour_threshold_percent))
                .ok_or_else(|| {
                    Error::numeric(format!(
                        "a quoted count of {} does not scale against the threshold; supply \
                         counts from a bounded window",
                        facts.quoted
                    ))
                })?;
            if honoured_scaled <= threshold_scaled {
                return Err(Error::denied(format!(
                    "this venue honoured {} of {} quotes and the threshold is {} per cent; path \
                     6 is the one path whose completion is a counterparty's promise, so a venue \
                     that does not exceed the threshold is routed another way",
                    facts.honoured, facts.quoted, facts.honour_threshold_percent
                )));
            }
            Ok(ExtensionVerdict::with_checks(
                path,
                vec![CHECK_WINDOW_EXCEEDS_BUDGET, CHECK_HONOUR_RATE],
            ))
        }

        ExecutionPath::RepresentationBasis => {
            let facts = extensions.basis.ok_or_else(|| {
                Error::invalid(
                    "path 7 was assigned and no basis facts were supplied; §33.1 requires the \
                     expected carry to exceed financing plus capital-occupancy cost to \
                     convergence, so supply the three figures to convergence",
                )
            })?;
            let cost = facts
                .financing
                .checked_add(facts.capital_occupancy)
                .ok_or_else(|| {
                    Error::numeric(format!(
                        "financing of {} and capital occupancy of {} do not sum to a Decimal",
                        facts.financing, facts.capital_occupancy
                    ))
                })?;
            if facts.expected_carry <= cost {
                return Err(Error::denied(format!(
                    "the expected carry to convergence is {} and financing plus capital \
                     occupancy is {cost}; path 7 holds the position until the two \
                     representations converge, and a carry that does not cover the cost of \
                     holding it is a loss taken slowly",
                    facts.expected_carry
                )));
            }
            Ok(ExtensionVerdict::with_checks(
                path,
                vec![CHECK_CARRY_COVERS_COST],
            ))
        }

        ExecutionPath::PayoffEquivalence => {
            let facts = extensions.payoff.ok_or_else(|| {
                Error::invalid(
                    "path 8 was assigned and no payoff facts were supplied; §33.1 requires net \
                     delta, gamma and vega inside limits and either a European-style structure \
                     or an explicit assignment budget, so supply the three exposures, their \
                     limits and the exercise style",
                )
            })?;
            for (name, value, limit) in [
                ("delta", facts.net_delta, facts.delta_limit),
                ("gamma", facts.net_gamma, facts.gamma_limit),
                ("vega", facts.net_vega, facts.vega_limit),
            ] {
                if value.abs() > limit {
                    return Err(Error::denied(format!(
                        "net {name} is {value} against a limit of {limit}; a payoff cycle is \
                         only an arbitrage while its residual exposure stays inside the limits \
                         the desk set, and outside them it is a position nobody sized"
                    )));
                }
            }
            if !facts.european_style && facts.assignment_budget.is_none() {
                return Err(Error::denied(
                    "this structure is not European-style and no assignment budget was stated; \
                     an American-style leg can be assigned before convergence and the \
                     replication then breaks at the worst moment — state the budget the desk \
                     will carry for early assignment, or trade the European form",
                ));
            }
            Ok(ExtensionVerdict::with_checks(
                path,
                vec![CHECK_GREEKS_INSIDE_LIMITS, CHECK_EXERCISE_STYLE],
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn d(literal: &str) -> Decimal {
        Decimal::parse(literal).expect("a test decimal parses")
    }

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn band() -> InventoryBand {
        InventoryBand::new(d("100"), d("5"), d("20")).expect("a valid band")
    }

    fn reference() -> DistributedReference {
        DistributedReference::new(d("50000"), d("100"), t(1_000), Duration::from_secs(60))
            .expect("a valid reference")
    }

    /// A region below target, seeing a local price below the reference: the
    /// band permits a buy and the reference indicates one.
    fn permitted_mirror() -> MirrorExtension {
        MirrorExtension::new(band(), d("90"), reference(), d("49800"), Direction::Buy)
    }

    #[test]
    fn paths_one_and_two_are_reported_as_having_no_row_rather_than_as_having_passed_a_check() {
        // The distinction is the whole point: "the blueprint asks for
        // nothing here" and "the platform checked nothing" read the same in
        // a log that records only success, and they are opposite facts.
        for path in [ExecutionPath::IntraVenue, ExecutionPath::CrossVenue] {
            let verdict =
                check(path, &PathExtensions::new(), t(1_010)).expect("no row is not a refusal");
            assert!(!verdict.has_row());
            assert!(verdict.satisfied().is_empty());
            assert!(
                verdict.rationale().contains("names no additional check"),
                "the rationale should say the table has no row: {}",
                verdict.rationale()
            );
        }
    }

    #[test]
    fn every_path_with_a_row_refuses_when_its_facts_are_absent() {
        // The failure this prevents is the one §33.1 is most exposed to: an
        // extension that treats a missing fact as nothing to check. Six
        // rows, six refusals, and the count is asserted so that a path
        // quietly moved into the no-row arm fails here.
        let empty = PathExtensions::new();
        let mut refused: BTreeSet<&'static str> = BTreeSet::new();
        for path in ExecutionPath::ALL {
            match check(path, &empty, t(1_010)) {
                Ok(verdict) => assert!(
                    !verdict.has_row(),
                    "path {} has a row in §33.1 and passed with no facts at all",
                    path.number()
                ),
                Err(refusal) => {
                    assert_eq!(refusal.code(), "invalid");
                    assert!(
                        refusal.message().contains("no ") && refusal.message().contains("supplied"),
                        "the refusal should name the missing facts: {}",
                        refusal.message()
                    );
                    refused.insert(path.as_str());
                }
            }
        }
        assert_eq!(
            refused.len(),
            6,
            "§33.1 names six rows and {} refused for missing facts: {refused:?}",
            refused.len()
        );
    }

    #[test]
    fn path_three_requires_the_band_and_the_reference_both_every_time() {
        // §33.1's row 3 ends "Both, every time". Each half is removed in
        // turn and the check must refuse either way; a gate that passed on
        // one of the two would be half a control.
        let now = t(1_010);
        let permitted = check(
            ExecutionPath::MirroredInventory,
            &PathExtensions::new().with_mirror(permitted_mirror()),
            now,
        )
        .expect("below target with the reference indicating a buy");
        assert_eq!(
            permitted.satisfied(),
            [CHECK_REFERENCE_FRESH, CHECK_DIRECTION_PERMITTED]
        );
        assert_eq!(
            permitted
                .mirror()
                .expect("path 3's verdict carries the permission")
                .direction(),
            Direction::Buy
        );

        // Reference outside its window, band unchanged.
        let stale = check(
            ExecutionPath::MirroredInventory,
            &PathExtensions::new().with_mirror(permitted_mirror()),
            t(1_100),
        )
        .expect_err("a reference past its window gates nothing");
        assert_eq!(stale.code(), "denied");
        assert!(
            stale.message().contains("stopped republishing"),
            "the refusal should name the stale reference: {}",
            stale.message()
        );

        // Band forbids the direction, reference unchanged.
        let forbidden = check(
            ExecutionPath::MirroredInventory,
            &PathExtensions::new().with_mirror(MirrorExtension::new(
                band(),
                d("110"),
                reference(),
                d("49800"),
                Direction::Buy,
            )),
            now,
        )
        .expect_err("a region above target may not buy");
        assert_eq!(forbidden.code(), "denied");
        assert!(
            forbidden
                .message()
                .contains("may sell_only, so it may not buy"),
            "the refusal should name the band: {}",
            forbidden.message()
        );
    }

    #[test]
    fn a_region_at_target_is_refused_because_nothing_here_can_make_a_scanned_cycle_smaller() {
        // §31.1's third row is "either direction, reduced size", and this
        // gate has no sizing authority. Passing the cycle at full size would
        // be the gate reading as protection while permitting exactly what
        // the row narrows.
        let refusal = check(
            ExecutionPath::MirroredInventory,
            &PathExtensions::new().with_mirror(MirrorExtension::new(
                band(),
                d("101"),
                reference(),
                d("49800"),
                Direction::Buy,
            )),
            t(1_010),
        )
        .expect_err("at target the size must be reduced and cannot be");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal
                .message()
                .contains("at-target row permits either direction at reduced size"),
            "the refusal should name the row: {}",
            refusal.message()
        );
    }

    #[test]
    fn path_four_refuses_a_hedge_that_is_remembered_rather_than_read_and_one_that_is_too_thin() {
        let now = t(1_010);
        let ok = check(
            ExecutionPath::HedgedBridging,
            &PathExtensions::new()
                .with_hedge(HedgeExtension::new(d("10"), d("10"), true).expect("valid")),
            now,
        )
        .expect("depth exactly meets the requirement");
        assert_eq!(ok.satisfied(), [CHECK_HEDGE_AT_DEPTH]);

        let stale = check(
            ExecutionPath::HedgedBridging,
            &PathExtensions::new()
                .with_hedge(HedgeExtension::new(d("100"), d("10"), false).expect("valid")),
            now,
        )
        .expect_err("a hedge not read this pass is not a hedge");
        assert_eq!(stale.code(), "denied");
        assert!(
            stale.message().contains("was not read this pass"),
            "the refusal should name the staleness: {}",
            stale.message()
        );

        let thin = check(
            ExecutionPath::HedgedBridging,
            &PathExtensions::new()
                .with_hedge(HedgeExtension::new(d("9"), d("10"), true).expect("valid")),
            now,
        )
        .expect_err("a partial hedge is not a hedge");
        assert_eq!(thin.code(), "denied");
        assert!(
            thin.message().contains("a partial hedge leaves"),
            "the refusal should name the gap: {}",
            thin.message()
        );
    }

    #[test]
    fn path_five_refuses_when_nothing_is_resting_and_when_the_spread_does_not_cover_the_premium() {
        let now = t(1_010);
        let ok = check(
            ExecutionPath::PassiveAnchoring,
            &PathExtensions::new()
                .with_anchor(AnchorExtension::new(true, d("5"), d("5")).expect("valid")),
            now,
        )
        .expect("a spread exactly covering the premium covers it");
        assert_eq!(ok.satisfied(), [CHECK_RESTING_LIVE, CHECK_PREMIUM_COVERED]);

        let nothing_resting = check(
            ExecutionPath::PassiveAnchoring,
            &PathExtensions::new()
                .with_anchor(AnchorExtension::new(false, d("50"), d("5")).expect("valid")),
            now,
        )
        .expect_err("no resting order means no anchor");
        assert_eq!(nothing_resting.code(), "denied");
        assert!(
            nothing_resting
                .message()
                .contains("no resting order is live"),
            "the refusal should say what is missing: {}",
            nothing_resting.message()
        );

        let uncovered = check(
            ExecutionPath::PassiveAnchoring,
            &PathExtensions::new()
                .with_anchor(AnchorExtension::new(true, d("4"), d("5")).expect("valid")),
            now,
        )
        .expect_err("a premium beyond the spread is a loss");
        assert_eq!(uncovered.code(), "denied");
        assert!(
            uncovered.message().contains("in adverse selection"),
            "the refusal should name both figures: {}",
            uncovered.message()
        );
    }

    #[test]
    fn path_six_refuses_a_window_that_does_not_beat_the_whole_budget_and_a_venue_at_the_threshold()
    {
        let now = t(1_010);
        let facts = |window_ms: i64, honoured: u64| {
            FirmQuoteExtension::new(
                Duration::from_millis(window_ms),
                Duration::from_millis(28),
                Duration::from_millis(5),
                Duration::from_millis(2),
                honoured,
                100,
                95,
            )
            .expect("valid firm-quote facts")
        };

        let ok = check(
            ExecutionPath::FirmQuoteBridging,
            &PathExtensions::new().with_firm_quote(facts(36, 96)),
            now,
        )
        .expect("a window one millisecond beyond the budget, honoured above the threshold");
        assert_eq!(
            ok.satisfied(),
            [CHECK_WINDOW_EXCEEDS_BUDGET, CHECK_HONOUR_RATE]
        );

        // 28 + 5 + 2 = 35. A window of exactly 35 arrives as the quote dies.
        let exact = check(
            ExecutionPath::FirmQuoteBridging,
            &PathExtensions::new().with_firm_quote(facts(35, 96)),
            now,
        )
        .expect_err("a window equal to the budget has no margin left");
        assert_eq!(exact.code(), "denied");
        assert!(
            exact.message().contains("sum to 35 ms"),
            "the refusal should name the budget it computed: {}",
            exact.message()
        );

        // 95 of 100 is exactly the threshold, and §33.1 says "above".
        let at_threshold = check(
            ExecutionPath::FirmQuoteBridging,
            &PathExtensions::new().with_firm_quote(facts(36, 95)),
            now,
        )
        .expect_err("exactly the threshold is not above it");
        assert_eq!(at_threshold.code(), "denied");
        assert!(
            at_threshold.message().contains("honoured 95 of 100 quotes"),
            "the refusal should name the record: {}",
            at_threshold.message()
        );
    }

    #[test]
    fn a_venue_nobody_has_seen_quote_has_no_honour_rate_and_is_refused_rather_than_assumed_perfect()
    {
        let refusal = FirmQuoteExtension::new(
            Duration::from_millis(100),
            Duration::from_millis(28),
            Duration::from_millis(5),
            Duration::from_millis(2),
            0,
            0,
            95,
        )
        .expect_err("no observation is not a perfect record");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("has not earned it"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn path_seven_refuses_a_carry_that_only_equals_the_cost_of_holding_the_position() {
        let now = t(1_010);
        let ok = check(
            ExecutionPath::RepresentationBasis,
            &PathExtensions::new()
                .with_basis(BasisExtension::new(d("101"), d("60"), d("40")).expect("valid")),
            now,
        )
        .expect("a carry one unit beyond the cost");
        assert_eq!(ok.satisfied(), [CHECK_CARRY_COVERS_COST]);

        let equal = check(
            ExecutionPath::RepresentationBasis,
            &PathExtensions::new()
                .with_basis(BasisExtension::new(d("100"), d("60"), d("40")).expect("valid")),
            now,
        )
        .expect_err("a carry equal to the cost earns nothing and carries the risk");
        assert_eq!(equal.code(), "denied");
        assert!(
            equal.message().contains("a loss taken slowly"),
            "the refusal should say what it is: {}",
            equal.message()
        );
    }

    #[test]
    fn path_seven_refuses_a_negative_cost_that_would_let_an_uncovered_carry_pass() {
        // A cost stated as a negative would add to the carry rather than
        // subtracting from it, and a basis cycle that does not cover its
        // financing would pass the one check that exists to stop it.
        let refusal = BasisExtension::new(d("10"), d("-100"), d("0"))
            .expect_err("a negative financing cost is a payment received");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal
                .message()
                .contains("a payment received rather than a cost"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn path_eight_refuses_an_exposure_past_a_limit_and_an_american_structure_with_no_budget() {
        let now = t(1_010);
        let facts = |vega: &str, european: bool, budget: Option<Decimal>| {
            PayoffExtension::new(
                d("1"),
                d("1"),
                d(vega),
                d("10"),
                d("10"),
                d("10"),
                european,
                budget,
            )
            .expect("valid payoff facts")
        };

        let ok = check(
            ExecutionPath::PayoffEquivalence,
            &PathExtensions::new().with_payoff(facts("10", true, None)),
            now,
        )
        .expect("an exposure exactly at the limit is inside it");
        assert_eq!(
            ok.satisfied(),
            [CHECK_GREEKS_INSIDE_LIMITS, CHECK_EXERCISE_STYLE]
        );

        let past = check(
            ExecutionPath::PayoffEquivalence,
            &PathExtensions::new().with_payoff(facts("-11", true, None)),
            now,
        )
        .expect_err("a limit is on the absolute exposure, so a short vega breaches it too");
        assert_eq!(past.code(), "denied");
        assert!(
            past.message().contains("net vega is -11"),
            "the refusal should name the exposure: {}",
            past.message()
        );

        let american = check(
            ExecutionPath::PayoffEquivalence,
            &PathExtensions::new().with_payoff(facts("1", false, None)),
            now,
        )
        .expect_err("an American structure with no budget can be assigned early");
        assert_eq!(american.code(), "denied");
        assert!(
            american
                .message()
                .contains("no assignment budget was stated"),
            "the refusal should say what is missing: {}",
            american.message()
        );

        // And the stated budget is the alternative §33.1 names, so the same
        // structure passes with one.
        assert!(
            check(
                ExecutionPath::PayoffEquivalence,
                &PathExtensions::new().with_payoff(facts("1", false, Some(d("5000")))),
                now,
            )
            .is_ok()
        );
    }

    #[test]
    fn an_assignment_budget_of_nothing_is_refused_rather_than_read_as_the_alternative_to_european()
    {
        // §33.1's row 8 ends "European-style **or** explicit assignment
        // budget", so a stated budget is what lets an American structure
        // through. A budget of zero is no allowance at all wearing the
        // clothes of one, and admitting it would turn the only alternative
        // the section names into a field a caller sets to satisfy the check.
        let refusal = PayoffExtension::new(
            d("1"),
            d("1"),
            d("1"),
            d("10"),
            d("10"),
            d("10"),
            false,
            Some(Decimal::ZERO),
        )
        .expect_err("a budget of zero is not a budget");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("is not a budget"),
            "the refusal should say why: {}",
            refusal.message()
        );
        // And a real budget is admitted, so the guard is not refusing every
        // American structure by another route.
        assert!(
            PayoffExtension::new(
                d("1"),
                d("1"),
                d("1"),
                d("10"),
                d("10"),
                d("10"),
                false,
                Some(d("5000")),
            )
            .is_ok()
        );
    }

    #[test]
    fn every_other_refusal_a_row_s_constructor_makes_is_reachable_from_an_input() {
        // The remaining guards, each with the input that reaches it and the
        // phrase that names it. A constructor refusal nothing supplies an
        // input for is a control that reads as protection and has never been
        // shown to fire — which is this repository's named failure mode, and
        // the reason this test is a table rather than a paragraph of prose
        // about validation.
        let ms = Duration::from_millis;
        let refusals: Vec<(&'static str, Error)> = vec![
            (
                "makes any depth sufficient",
                HedgeExtension::new(d("10"), Decimal::ZERO, true).expect_err("zero required"),
            ),
            (
                "is not a depth",
                HedgeExtension::new(d("-1"), d("10"), true).expect_err("negative depth"),
            ),
            (
                "is crossed, not a spread",
                AnchorExtension::new(true, d("-1"), d("1")).expect_err("negative spread"),
            ),
            (
                "is a payment for resting",
                AnchorExtension::new(true, d("1"), d("-1")).expect_err("negative premium"),
            ),
            (
                "more honoured than offered",
                FirmQuoteExtension::new(ms(100), ms(28), ms(5), ms(2), 101, 100, 95)
                    .expect_err("more honoured than quoted"),
            ),
            (
                "can never be exceeded",
                FirmQuoteExtension::new(ms(100), ms(28), ms(5), ms(2), 99, 100, 101)
                    .expect_err("a threshold past a hundred per cent"),
            ),
            (
                "a firm-quote window of 0 ns is not a measurement",
                FirmQuoteExtension::new(Duration::ZERO, ms(28), ms(5), ms(2), 99, 100, 95)
                    .expect_err("an unmeasured window"),
            ),
            (
                "a round trip of 0 ns is not a measurement",
                FirmQuoteExtension::new(ms(100), Duration::ZERO, ms(5), ms(2), 99, 100, 95)
                    .expect_err("an unmeasured round trip"),
            ),
            (
                "an execution time of 0 ns is not a measurement",
                FirmQuoteExtension::new(ms(100), ms(28), Duration::ZERO, ms(2), 99, 100, 95)
                    .expect_err("an unmeasured execution time"),
            ),
            (
                "spends time the budget does not have",
                FirmQuoteExtension::new(
                    ms(100),
                    ms(28),
                    ms(5),
                    Duration::from_nanos(-1),
                    99,
                    100,
                    95,
                )
                .expect_err("a negative margin"),
            ),
            (
                "a net delta limit of -1 can never be satisfied",
                PayoffExtension::new(
                    d("1"),
                    d("1"),
                    d("1"),
                    d("-1"),
                    d("10"),
                    d("10"),
                    true,
                    None,
                )
                .expect_err("a negative limit"),
            ),
        ];
        // Premise: every one of them really did refuse, and refused as an
        // invalid input rather than as something else.
        assert_eq!(refusals.len(), 11);
        for (phrase, refusal) in &refusals {
            assert_eq!(refusal.code(), "invalid", "{}", refusal.message());
            assert!(
                refusal.message().contains(phrase),
                "the refusal should name {phrase:?}: {}",
                refusal.message()
            );
        }
        // And the good value each guard sits beside, so none of them is a
        // constructor that refuses everything.
        assert!(HedgeExtension::new(Decimal::ZERO, d("10"), true).is_ok());
        assert!(AnchorExtension::new(true, Decimal::ZERO, Decimal::ZERO).is_ok());
        assert!(
            FirmQuoteExtension::new(ms(100), ms(28), ms(5), Duration::ZERO, 100, 100, 100).is_ok()
        );
        assert!(
            PayoffExtension::new(
                d("1"),
                d("1"),
                d("1"),
                Decimal::ZERO,
                d("10"),
                d("10"),
                true,
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn every_one_of_the_eight_paths_has_an_arm_that_can_return_a_verdict() {
        // The guard against an arm nobody can reach. If a path could only
        // ever refuse, §33.1's row for it would be a check that admits
        // nothing — the mirror image of a check that refuses nothing, and
        // just as useless.
        let now = t(1_010);
        let all = PathExtensions::new()
            .with_mirror(permitted_mirror())
            .with_hedge(HedgeExtension::new(d("100"), d("10"), true).expect("valid"))
            .with_anchor(AnchorExtension::new(true, d("50"), d("5")).expect("valid"))
            .with_firm_quote(
                FirmQuoteExtension::new(
                    Duration::from_millis(100),
                    Duration::from_millis(28),
                    Duration::from_millis(5),
                    Duration::from_millis(2),
                    99,
                    100,
                    95,
                )
                .expect("valid"),
            )
            .with_basis(BasisExtension::new(d("500"), d("60"), d("40")).expect("valid"))
            .with_payoff(
                PayoffExtension::new(
                    d("1"),
                    d("1"),
                    d("1"),
                    d("10"),
                    d("10"),
                    d("10"),
                    true,
                    None,
                )
                .expect("valid"),
            );
        let mut seen: BTreeSet<u8> = BTreeSet::new();
        for path in ExecutionPath::ALL {
            let verdict = check(path, &all, now)
                .unwrap_or_else(|error| panic!("path {} refused: {}", path.number(), error));
            assert_eq!(verdict.path(), path);
            seen.insert(path.number());
        }
        assert_eq!(seen.len(), 8, "only {} of eight paths passed", seen.len());
    }
}
