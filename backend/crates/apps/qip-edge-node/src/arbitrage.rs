//! Installing the arbitrage desk into a deployed cell.
//!
//! The scanner was wired into the cell at the seam §27.2 names, and no
//! composition root gave a cell a desk, so every cycle in the tree was still
//! found by a test and taken by nobody. The reason was the source: the desk
//! needs a graph, and the payload's cycle whitelist carried strings. This
//! module is the other half. The whitelist now carries its trade edges in
//! structured form (`CycleWhitelist::conversions`), the desk's capital rides
//! the same verified grant channel every strategy's does, and this installer
//! waits until it holds both and the cell is not degraded, then builds the
//! desk once.
//!
//! # What refuses, and where
//!
//! Every edge is checked against the cell's own venue list before a graph
//! exists: a conversion naming a venue this cell may not trade refuses the
//! whole whitelist, because a cycle through a venue the cell holds no book
//! for cannot be priced here, and `Cell::install_arbitrage` would refuse the
//! finished graph anyway — refusing at the entry names the entry. Every
//! start instrument must have a size, or the scanner refuses that cycle as
//! unsized on every pass and the desk reads as quiet rather than as
//! misconfigured. A degraded cell installs nothing: the desk would refuse to
//! scan under a narrowed multiplier, and a desk installed to do nothing is a
//! desk an operator reads as working.
//!
//! # What is fixed here rather than shipped
//!
//! The cap on cycles per pass and the leg validity are constants of this
//! node. They bound how much one pass can commit and how long a leg may
//! wait at the venue; neither is a fact the centre knows better than the
//! node that sends the orders, and shipping them would put two claims about
//! the same bound in two places.

use qip_arbitrage::graph::VenueFacts;
use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy,
};
use qip_contracts::degradation::StrategyClass;
use qip_contracts::policy::CycleWhitelist;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::Cell;
use qip_edge::envelope::VerifiedEnvelope;
use std::collections::BTreeMap;

/// The environment variable naming the desk's strategy, whose grant funds
/// it. Unset is a node that installs no desk, and says so.
pub const STRATEGY_VARIABLE: &str = "QIP_ARBITRAGE_STRATEGY";

/// How many surviving cycles one pass may commit. Small on purpose: every
/// cycle is up to four legs the cell cannot unwind, and the next pass finds
/// what this one refused if it is still there.
pub const MAX_CYCLES_PER_PASS: usize = 4;

/// How long a leg may wait at the venue before it is stale.
pub const LEG_VALIDITY: Duration = Duration::from_secs(30);

/// The planner's notional budget for one cycle's legs.
///
/// A bound on the planner, not on capital: capital is the desk's envelope,
/// which the centre signed. This keeps a single planned cycle from being
/// sized past what one pass should ever send, whatever the envelope holds.
const PLAN_BUDGET: &str = "50000";

/// The most conversions a whitelist may carry. The graph is walked on every
/// pass; a whitelist past this is not a whitelist but a market.
pub const MAX_CONVERSIONS: usize = 256;

/// Build the graph a whitelist describes, against the venues this cell may
/// trade.
///
/// Every trade edge is added at a placeholder rate of one: the graph refuses
/// zero, and the desk re-quotes every trade edge from the cell's books before
/// every scan, so the placeholder is a value no scan reads. The venue's
/// facts come from the whitelist and its status is `Open` at installation —
/// the cell judges each leg against the book's real status when it sends.
pub fn graph_from_whitelist(
    whitelist: &CycleWhitelist,
    venues: &[VenueId],
) -> Result<ArbitrageGraph> {
    if whitelist.conversions.is_empty() {
        return Err(Error::invalid(
            "the cycle whitelist carries no conversion, so there is no graph to build",
        ));
    }
    if whitelist.conversions.len() > MAX_CONVERSIONS {
        return Err(Error::invalid(format!(
            "the cycle whitelist carries {} conversions and this node walks at most \
             {MAX_CONVERSIONS} per pass",
            whitelist.conversions.len()
        )));
    }
    let mut graph = ArbitrageGraph::new();
    let mut classes = BTreeMap::new();
    for (position, conversion) in whitelist.conversions.iter().enumerate() {
        let venue = VenueId::new(conversion.venue.as_str());
        if !venues.contains(&venue) {
            return Err(Error::denied(format!(
                "conversion {position} ({} -> {} at {}) names a venue this cell may not trade; \
                 a cycle through a venue the cell holds no book for cannot be priced here, \
                 and the whitelist is refused whole",
                conversion.from, conversion.to, conversion.venue
            )));
        }
        if conversion.from == conversion.to {
            return Err(Error::invalid(format!(
                "conversion {position} converts {} into itself",
                conversion.from
            )));
        }
        if conversion.cost_fraction.is_negative() || conversion.cost_fraction >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "conversion {position} has cost fraction {}, outside [0, 1)",
                conversion.cost_fraction
            )));
        }
        match classes.insert(venue.clone(), conversion.venue_class) {
            Some(previous) if previous != conversion.venue_class => {
                return Err(Error::invalid(format!(
                    "the whitelist names {} as both {previous:?} and {:?}; a venue has one \
                     class, and the planner's settlement assumptions depend on which",
                    conversion.venue, conversion.venue_class
                )));
            }
            _ => {}
        }
        graph.register_venue(
            venue.clone(),
            VenueFacts::new(conversion.venue_class, VenueStatus::Open),
        );
        graph.add_trade(
            Node::new(
                ObjectId::from_string(conversion.from.as_str()),
                venue.clone(),
            ),
            Node::new(ObjectId::from_string(conversion.to.as_str()), venue),
            Decimal::ONE,
            conversion.cost_fraction,
            ObjectId::from_string(conversion.market.as_str()),
            conversion.side,
            Timestamp::EPOCH,
            0,
        )?;
    }
    Ok(graph)
}

/// The sizes a whitelist names, refusing a start instrument it leaves
/// unsized.
pub fn sizes_from_whitelist(whitelist: &CycleWhitelist) -> Result<SizePolicy> {
    let mut sizes = SizePolicy::default();
    for (object, size) in &whitelist.start_sizes {
        if !size.is_positive() {
            return Err(Error::invalid(format!(
                "the start size for {object} is {size}; a size must be positive"
            )));
        }
        sizes = sizes.with(ObjectId::from_string(object.as_str()), *size);
    }
    for conversion in &whitelist.conversions {
        if !whitelist.start_sizes.contains_key(&conversion.from) {
            return Err(Error::invalid(format!(
                "the whitelist names no start size for {}; a cycle from an unsized instrument \
                 is refused by the scanner on every pass and the desk reads as quiet",
                conversion.from
            )));
        }
    }
    Ok(sizes)
}

/// Assemble the desk a whitelist describes, funded by `envelope`.
pub fn desk_from_whitelist(
    whitelist: &CycleWhitelist,
    venues: &[VenueId],
    strategy: StrategyId,
    envelope: VerifiedEnvelope,
) -> Result<ArbitrageDesk> {
    let graph = graph_from_whitelist(whitelist, venues)?;
    let sizes = sizes_from_whitelist(whitelist)?;
    let budget = Decimal::parse(PLAN_BUDGET)
        .ok_or_else(|| Error::invalid("the plan budget literal is not a decimal"))?;
    let scanner = OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(budget),
    );
    ArbitrageDesk::new(
        strategy,
        scanner,
        graph,
        sizes,
        envelope,
        MAX_CYCLES_PER_PASS,
        LEG_VALIDITY,
    )
}

/// What one attempt to install the desk did, for the tick and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installation {
    /// The cell already holds a desk.
    AlreadyInstalled,
    /// No fresh whitelist has been applied.
    NoWhitelist,
    /// The whitelist carries no conversion.
    EmptyWhitelist,
    /// The cell is narrowed, and a desk would refuse to scan.
    Degraded,
    /// No verified grant for the desk's strategy has arrived.
    NoEnvelope,
    /// Installed, with this many trade edges.
    Installed(usize),
    /// The whitelist or the cell refused, and the envelope is kept for a
    /// whitelist that does not.
    Refused(String),
}

impl Installation {
    /// Whether an operator needs to look.
    pub const fn is_quiet(&self) -> bool {
        !matches!(self, Self::Installed(_) | Self::Refused(_))
    }

    pub fn describe(&self) -> String {
        match self {
            Self::AlreadyInstalled => "already installed".to_string(),
            Self::NoWhitelist => "no fresh cycle whitelist applied".to_string(),
            Self::EmptyWhitelist => "the cycle whitelist carries no conversion".to_string(),
            Self::Degraded => "the cell is degraded and a desk would not scan".to_string(),
            Self::NoEnvelope => "no verified grant for the desk's strategy has arrived".to_string(),
            Self::Installed(edges) => format!("installed with {edges} trade edge(s)"),
            Self::Refused(reason) => format!("refused: {reason}"),
        }
    }
}

/// Holds the desk's grant until the whitelist that spends it arrives, and
/// installs once.
///
/// Bounded at one envelope: a newer grant for the desk's strategy replaces
/// the held one, exactly as `Cell::renew_capital` replaces a deployed
/// strategy's, and nothing accumulates.
#[derive(Debug)]
pub struct ArbitrageInstaller {
    strategy: StrategyId,
    venues: Vec<VenueId>,
    envelope: Option<VerifiedEnvelope>,
}

impl ArbitrageInstaller {
    pub fn new(strategy: StrategyId, venues: Vec<VenueId>) -> Self {
        Self {
            strategy,
            venues,
            envelope: None,
        }
    }

    pub fn strategy(&self) -> &StrategyId {
        &self.strategy
    }

    pub fn holds_envelope(&self) -> bool {
        self.envelope.is_some()
    }

    /// Hold a verified grant for the desk's strategy.
    ///
    /// Refuses one for any other strategy: this is not a place a grant for
    /// something undeployed may wait, because a grant that waits is a grant
    /// that may be spent later by whatever is deployed under that name.
    pub fn offer(&mut self, envelope: VerifiedEnvelope) -> Result<()> {
        if envelope.strategy() != &self.strategy {
            return Err(Error::denied(format!(
                "a grant for strategy {} cannot fund the arbitrage desk {}",
                envelope.strategy().as_str(),
                self.strategy.as_str()
            )));
        }
        self.envelope = Some(envelope);
        Ok(())
    }

    /// Install the desk if everything it needs is present, and say what
    /// happened either way.
    pub fn install(&mut self, cell: &mut Cell, now: Timestamp) -> Installation {
        if cell.arbitrage().is_some() {
            return Installation::AlreadyInstalled;
        }
        let Some(whitelist) = cell.cycle_whitelist(now).cloned() else {
            return Installation::NoWhitelist;
        };
        if whitelist.conversions.is_empty() {
            return Installation::EmptyWhitelist;
        }
        let narrowing = cell.narrowing(now);
        if narrowing.pauses(StrategyClass::PriceOnly)
            || narrowing.sizing_multiplier() < Decimal::ONE
        {
            return Installation::Degraded;
        }
        let Some(envelope) = self.envelope.clone() else {
            return Installation::NoEnvelope;
        };
        let desk =
            match desk_from_whitelist(&whitelist, &self.venues, self.strategy.clone(), envelope) {
                Ok(desk) => desk,
                Err(error) => return Installation::Refused(error.message().to_string()),
            };
        let edges = desk.graph().edge_count();
        match cell.install_arbitrage(desk) {
            Ok(()) => {
                // Spent: the desk holds it now, and `renew_capital` replaces
                // it from here on as it does any strategy's.
                self.envelope = None;
                Installation::Installed(edges)
            }
            Err(error) => Installation::Refused(error.message().to_string()),
        }
    }
}
