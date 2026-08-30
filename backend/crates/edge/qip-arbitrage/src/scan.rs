//! The front door: graph in, executable opportunities and refusals out.
//!
//! The stages run in a fixed order, cheapest first, and each one can only
//! remove paths:
//!
//! 1. [`crate::search`] proposes cycles in log space.
//! 2. Exact arithmetic re-multiplies the quoted rates and drops what was
//!    rounding.
//! 3. [`crate::pricing`] walks the book at the stated size.
//! 4. [`crate::netedge`] takes off all nine deductions.
//! 5. [`crate::plan`] orders the legs and bounds what could be stranded.
//!
//! Every refusal is kept. A scan that returned nothing and said nothing about
//! why is indistinguishable from a scan that did not run, and the difference
//! matters at the point somebody asks why a known opportunity was not taken.

use crate::graph::ArbitrageGraph;
use crate::liquidity::LiquiditySource;
use crate::netedge::{EdgeAssumptions, NetEdgeCalculator};
use crate::plan::{LegPlanner, PlanSettings, PlannedTrade};
use crate::pricing::{PathPricing, price_path};
use crate::search::{
    ExactConfirmation, PathCandidate, SearchSettings, confirm_exact, search_candidates,
};
use qip_contracts::edge::NetEdge;
use qip_core::{Decimal, ObjectId, Timestamp};
use std::collections::BTreeMap;

/// How much of each instrument a scan is willing to commit.
///
/// There is no default size. Edge is not linear in size, so a path priced
/// without one is not a number anyone can act on, and a scan that invented a
/// size would be answering a question nobody asked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SizePolicy {
    default_size: Option<Decimal>,
    by_object: BTreeMap<ObjectId, Decimal>,
}

impl SizePolicy {
    /// The same size for every starting instrument.
    pub fn uniform(size: Decimal) -> Self {
        Self {
            default_size: Some(size),
            by_object: BTreeMap::new(),
        }
    }

    /// Override the size for one instrument.
    pub fn with(mut self, object: ObjectId, size: Decimal) -> Self {
        self.by_object.insert(object, size);
        self
    }

    pub fn size_for(&self, object: &ObjectId) -> Option<Decimal> {
        self.by_object
            .get(object)
            .copied()
            .or(self.default_size)
            .filter(|size| *size > Decimal::ZERO)
    }
}

/// Which stage threw a candidate out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectionStage {
    /// No size was stated for the instrument the cycle starts from.
    Unsized,
    /// The exact product of the quoted rates does not exceed one.
    ExactArithmetic,
    /// The path could not be priced against the book at all.
    Unpriceable,
    /// The book does not hold the size the path asked for.
    Depth,
    /// Profitable on the quoted rates, unprofitable on the book.
    Book,
    /// The deductions swallowed it.
    NetEdge,
    /// It could not be turned into a plan anyone should execute.
    Plan,
}

impl RejectionStage {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unsized => "unsized",
            Self::ExactArithmetic => "exact_arithmetic",
            Self::Unpriceable => "unpriceable",
            Self::Depth => "depth",
            Self::Book => "book",
            Self::NetEdge => "net_edge",
            Self::Plan => "plan",
        }
    }
}

/// A candidate that did not survive, and what stopped it.
#[derive(Clone, Debug, PartialEq)]
pub struct Rejection {
    pub candidate: PathCandidate,
    pub stage: RejectionStage,
    pub detail: String,
}

/// A path that survived every stage.
#[derive(Clone, Debug, PartialEq)]
pub struct Opportunity {
    pub candidate: PathCandidate,
    pub confirmation: ExactConfirmation,
    pub pricing: PathPricing,
    pub net_edge: NetEdge,
    pub planned: PlannedTrade,
}

impl Opportunity {
    /// What the platform expects to keep, after everything.
    pub fn net(&self) -> Decimal {
        self.net_edge.net()
    }
}

/// What one pass over the graph found, and what it refused.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanReport {
    pub opportunities: Vec<Opportunity>,
    pub rejections: Vec<Rejection>,
}

impl ScanReport {
    /// Refusals at one stage, for a monitor that watches a single failure mode.
    pub fn rejected_at(&self, stage: RejectionStage) -> Vec<&Rejection> {
        self.rejections
            .iter()
            .filter(|rejection| rejection.stage == stage)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.opportunities.is_empty()
    }
}

/// Runs the whole pipeline over one graph.
#[derive(Clone, Debug, PartialEq)]
pub struct OpportunityScanner {
    search: SearchSettings,
    calculator: NetEdgeCalculator,
    planner: LegPlanner,
}

impl OpportunityScanner {
    pub fn new(
        search: SearchSettings,
        assumptions: EdgeAssumptions,
        plan_settings: PlanSettings,
    ) -> Self {
        Self {
            search,
            calculator: NetEdgeCalculator::new(assumptions),
            planner: LegPlanner::new(plan_settings),
        }
    }

    pub fn search_settings(&self) -> &SearchSettings {
        &self.search
    }

    /// Find every executable opportunity in the graph, and record the rest.
    ///
    /// `now` is a parameter for the same reason it is everywhere else here: the
    /// uncertainty haircut depends on it, and a scan that read a clock would
    /// not replay.
    pub fn scan(
        &self,
        graph: &ArbitrageGraph,
        source: &dyn LiquiditySource,
        sizes: &SizePolicy,
        now: Timestamp,
    ) -> ScanReport {
        let mut report = ScanReport::default();
        for candidate in search_candidates(graph, &self.search) {
            match self.evaluate(graph, source, sizes, &candidate, now) {
                Ok(opportunity) => report.opportunities.push(opportunity),
                Err(rejection) => report.rejections.push(rejection),
            }
        }
        report.opportunities.sort_by(|a, b| {
            b.net()
                .cmp(&a.net())
                .then_with(|| a.candidate.edges.cmp(&b.candidate.edges))
        });
        report
    }

    fn evaluate(
        &self,
        graph: &ArbitrageGraph,
        source: &dyn LiquiditySource,
        sizes: &SizePolicy,
        candidate: &PathCandidate,
        now: Timestamp,
    ) -> std::result::Result<Opportunity, Rejection> {
        let reject = |stage: RejectionStage, detail: String| Rejection {
            candidate: candidate.clone(),
            stage,
            detail,
        };

        let confirmation = confirm_exact(graph, candidate).map_err(|error| {
            reject(RejectionStage::ExactArithmetic, error.message().to_string())
        })?;
        if !confirmation.is_profitable() {
            return Err(reject(
                RejectionStage::ExactArithmetic,
                format!(
                    "the log-space search saw a gain of {} but the exact product of the rates is {}",
                    candidate.log_gain_f64, confirmation.multiple
                ),
            ));
        }

        let start = graph
            .edge(candidate.edges[0])
            .map(|edge| edge.from.object.clone())
            .ok_or_else(|| {
                reject(
                    RejectionStage::Unpriceable,
                    "the candidate names a conversion that is not in the graph".to_string(),
                )
            })?;
        let size = sizes.size_for(&start).ok_or_else(|| {
            reject(
                RejectionStage::Unsized,
                format!("no size is stated for {}", start.as_str()),
            )
        })?;

        let pricing = price_path(graph, source, candidate, size)
            .map_err(|error| reject(RejectionStage::Unpriceable, error.message().to_string()))?;
        if !pricing.is_fully_available() {
            return Err(reject(
                RejectionStage::Depth,
                "the book does not hold the size this path was priced at; a partly filled cycle is a position, not a smaller trade".to_string(),
            ));
        }
        if !pricing.is_profitable_on_book() {
            return Err(reject(
                RejectionStage::Book,
                format!(
                    "the quoted rates return {} but the book returns {} on a size of {size}",
                    pricing.indicative_end_quantity, pricing.end_quantity
                ),
            ));
        }

        let net_edge = self
            .calculator
            .calculate(&pricing, now)
            .map_err(|error| reject(RejectionStage::NetEdge, error.message().to_string()))?;
        if !net_edge.is_positive() {
            return Err(reject(RejectionStage::NetEdge, net_edge.summarise()));
        }

        let planned = self
            .planner
            .plan(&pricing)
            .map_err(|error| reject(RejectionStage::Plan, error.message().to_string()))?;

        Ok(Opportunity {
            candidate: candidate.clone(),
            confirmation,
            pricing,
            net_edge,
            planned,
        })
    }
}
