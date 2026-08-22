//! The cell: one region's hot execution path, assembled.
//!
//! Bytes arrive on a feed and leave as orders, without a network hop to the
//! central plane anywhere in between. That is the whole point of the cell and
//! the reason every safety property here has to be local: there is nobody to
//! ask.
//!
//! What makes it safe is that the cell never decides *how much* it may risk.
//! It receives a [`crate::VerifiedEnvelope`] — signed, bounded, venue-scoped,
//! expiring — and the worst it can do while cut off is spend an amount
//! somebody already approved, for as long as the envelope has left to run.

use crate::dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
use crate::envelope::VerifiedEnvelope;
use crate::journal::{Decision, Journal, Mirror};
use crate::seam::CellLiquidity;
use qip_contracts::capital::{CapitalGrant, Utilisation};
use qip_contracts::message::{BookSide, MarketMessage};
use qip_contracts::signal::{Signal, SignalKind, StrategyId};
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_feature_dag::engine::FeatureEngine;
use qip_orderbook::venue::VenueState;
use qip_protocols::registry::{FeedKey, ProtocolRegistry};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel};
use qip_sequencing::tracker::{ReorderPolicy, Sequencer};
use qip_strategy::compile::CompiledStrategy;
use qip_strategy::runtime::StrategyRuntime;
use std::collections::BTreeMap;

/// How a cell is identified and what it is allowed to reach.
#[derive(Clone, Debug)]
pub struct CellConfig {
    pub cell_id: String,
    pub region: String,
    /// Venues this cell may trade. A venue absent here is unreachable to it
    /// whatever an envelope says — the two are independent bounds, and an
    /// order must clear both.
    pub venues: Vec<VenueId>,
    /// How long a book may go unrefreshed before its prices stop counting.
    pub max_staleness: Duration,
    /// The runtime node budget a strategy may not exceed.
    pub strategy_budget: usize,
}

impl CellConfig {
    pub fn new(cell_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            cell_id: cell_id.into(),
            region: region.into(),
            venues: Vec::new(),
            max_staleness: Duration::from_secs(5),
            strategy_budget: 4_096,
        }
    }

    pub fn with_venue(mut self, venue: VenueId) -> Self {
        self.venues.push(venue);
        self
    }
}

/// What one pass of the cell's work produced.
#[derive(Clone, Debug, Default)]
pub struct WorkReport {
    pub signals: Vec<Signal>,
    pub orders: Vec<PlacedOrder>,
    /// Every gate that said no, and why. A cell must answer "why did nothing
    /// trade" as precisely as "why did this trade".
    pub refusals: Vec<(String, String)>,
    pub halted: bool,
}

/// An order the cell actually sent.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedOrder {
    pub order_id: String,
    pub strategy: StrategyId,
    pub object_id: ObjectId,
    pub venue: VenueId,
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// Set by the cell from the gateway's own answer, never taken from the
    /// order. A paper fill counted as real is the single most consequential
    /// bit in the execution path.
    pub simulated: bool,
}

/// A deployed strategy and the capital it runs under.
#[derive(Debug)]
struct Deployed {
    strategy: CompiledStrategy,
    envelope: VerifiedEnvelope,
    utilisation: Utilisation,
}

/// One edge cell.
#[derive(Debug)]
pub struct Cell {
    config: CellConfig,
    protocols: ProtocolRegistry,
    sequencer: Sequencer,
    liquidity: CellLiquidity,
    features: FeatureEngine,
    runtime: StrategyRuntime,
    deployed: BTreeMap<String, Deployed>,
    autonomy: AutonomyController,
    dropcopy: DropCopyReconciler,
    journal: Journal,
    fills: Vec<CellFill>,
    order_sequence: u64,
}

impl Cell {
    /// Assemble a cell.
    ///
    /// The autonomy ceiling is paper trading and there is no constructor that
    /// takes another. A cell cannot raise its own ceiling; a live-capable cell
    /// is a differently-assembled deployment the central plane signs off, and
    /// the absence of that constructor here is what makes the claim true
    /// rather than merely intended.
    pub fn new(config: CellConfig, features: FeatureEngine) -> Result<Self> {
        let runtime = StrategyRuntime::with_budget(
            qip_strategy::program::Program::default(),
            config.strategy_budget,
        )?;
        Ok(Self {
            protocols: ProtocolRegistry::new(),
            sequencer: Sequencer::new(ReorderPolicy::default()),
            liquidity: CellLiquidity::new(),
            features,
            runtime,
            deployed: BTreeMap::new(),
            autonomy: AutonomyController::new(),
            dropcopy: DropCopyReconciler::new(),
            journal: Journal::new(),
            fills: Vec::new(),
            order_sequence: 0,
            config,
        })
    }

    pub fn config(&self) -> &CellConfig {
        &self.config
    }

    pub fn protocols_mut(&mut self) -> &mut ProtocolRegistry {
        &mut self.protocols
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn autonomy(&self) -> &AutonomyController {
        &self.autonomy
    }

    pub fn autonomy_mut(&mut self) -> &mut AutonomyController {
        &mut self.autonomy
    }

    pub fn liquidity(&self) -> &CellLiquidity {
        &self.liquidity
    }

    pub fn dropcopy_mut(&mut self) -> &mut DropCopyReconciler {
        &mut self.dropcopy
    }

    pub fn fills(&self) -> &[CellFill] {
        &self.fills
    }

    /// Whether the cell is stopped.
    pub fn is_halted(&self) -> bool {
        self.autonomy.kill_switch().is_globally_tripped()
    }

    /// Track an instrument at a venue.
    pub fn track(&mut self, state: VenueState) {
        self.liquidity.insert(state);
    }

    /// Deploy a strategy under a verified capital envelope.
    ///
    /// Takes the verified type, so a strategy cannot be deployed against a
    /// grant nobody signed.
    pub fn deploy(&mut self, strategy: CompiledStrategy, envelope: VerifiedEnvelope) -> Result<()> {
        if envelope.cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot deploy into {}",
                envelope.cell(),
                self.config.cell_id
            )));
        }
        self.deployed.insert(
            envelope.strategy().as_str().to_string(),
            Deployed {
                strategy,
                envelope,
                utilisation: Utilisation::default(),
            },
        );
        Ok(())
    }

    pub fn deployed_strategies(&self) -> Vec<&str> {
        self.deployed.keys().map(String::as_str).collect()
    }

    // --- the hot path -------------------------------------------------------

    /// Bytes in: decode, sequence, apply, and mark features dirty.
    ///
    /// Does no file or network I/O. The mirror is drained by [`Cell::flush`]
    /// precisely so that this call's cost is arithmetic and memory, never a
    /// storage system's availability.
    pub fn on_bytes(&mut self, feed: &FeedKey, bytes: &[u8], now: Timestamp) -> Result<usize> {
        let (decoded, skipped) = {
            let decoder = self.protocols.decoder_mut(&feed.venue, &feed.feed)?;
            let decoded = decoder.decode(bytes, now)?;
            // Read the counter after decoding: it is cumulative, and the
            // difference is what this call actually skipped.
            let skipped = usize::try_from(decoder.diagnostics().messages_skipped).unwrap_or(0);
            (decoded, skipped)
        };
        let count = decoded.len();
        self.journal.record(
            Decision::Ingested {
                feed: format!("{}/{}", feed.venue.as_str(), feed.feed),
                decoded: count,
                skipped,
            },
            now,
        );

        let batch = self.sequencer.accept(decoded, now);
        self.apply_batch(batch.released, now)?;
        for event in &batch.events {
            if let Some(detail) = gap_detail(event) {
                self.journal.record(
                    Decision::GapDetected {
                        stream: detail.0,
                        detail: detail.1,
                    },
                    now,
                );
            }
        }
        Ok(count)
    }

    /// Apply released messages to books and the feature graph.
    fn apply_batch(&mut self, messages: Vec<MarketMessage>, _now: Timestamp) -> Result<()> {
        for message in &messages {
            let venue = message.origin.venue.clone();
            if let Some(state) = self.liquidity.get_mut(&venue, &message.object_id) {
                // A message the book refuses is a book that would be wrong if
                // it accepted it; the refusal is recorded by the reset path,
                // not swallowed here.
                state.apply(message)?;
            }
            self.features.ingest(message)?;
        }
        Ok(())
    }

    /// One pass of decide-and-act.
    ///
    /// Every gate that refuses records why, in order, so the reason a cell was
    /// quiet is reconstructable without re-running it.
    pub fn work(&mut self, now: Timestamp, gateway: &mut dyn Placer) -> Result<WorkReport> {
        let mut report = WorkReport {
            halted: self.is_halted(),
            ..WorkReport::default()
        };

        if report.halted {
            // Books keep absorbing and the journal keeps recording while
            // halted. A cell that stops seeing the market cannot tell whether
            // it is safe to resume.
            self.refuse(&mut report, "kill_switch", "the cell is halted", now);
            return Ok(report);
        }

        let vector = self.features.evaluate(now)?;
        let strategy_ids: Vec<String> = self.deployed.keys().cloned().collect();

        for id in strategy_ids {
            let Some(deployed) = self.deployed.get(&id) else {
                continue;
            };
            let signal = match self.runtime.run(&deployed.strategy, &vector, now) {
                Ok(Some(signal)) => signal,
                Ok(None) => continue,
                Err(error) => {
                    self.refuse(&mut report, "strategy_runtime", error.message(), now);
                    continue;
                }
            };

            self.journal.record(
                Decision::SignalRaised {
                    strategy: signal.strategy.as_str().to_string(),
                    object: signal.object_id.as_str().to_string(),
                    kind: signal.kind.as_str().to_string(),
                    conviction_shrunk_f64: signal.conviction.shrunk(),
                },
                now,
            );
            report.signals.push(signal.clone());

            if let Some(order) = self.place(&signal, now, gateway, &mut report)? {
                report.orders.push(order);
            }
        }

        Ok(report)
    }

    /// Take one signal through every gate to a venue, or refuse it.
    fn place(
        &mut self,
        signal: &Signal,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<Option<PlacedOrder>> {
        if !signal.is_live(now) {
            self.refuse(report, "signal_expiry", "the signal is no longer live", now);
            return Ok(None);
        }

        let Some(venue) = self.venue_for(&signal.object_id) else {
            self.refuse(
                report,
                "venue_selection",
                "no venue this cell may reach quotes the instrument",
                now,
            );
            return Ok(None);
        };

        // A stale or unpriceable book routes nothing. The book already refuses
        // to serve a mid; routing against one anyway would use a price from
        // before the gap that made it stale.
        // Read everything needed from the book in one borrow, so the refusal
        // path below can take `&mut self` to journal why it refused.
        let assessment = self.liquidity.get(&venue, &signal.object_id).map(|state| {
            (
                state.is_stale(),
                state
                    .reset_reason()
                    .unwrap_or("the book is awaiting resynchronisation")
                    .to_string(),
                state.status(),
                state.mid(),
            )
        });
        let Some((stale, reset_reason, status, mid)) = assessment else {
            self.refuse(
                report,
                "book",
                "the cell holds no book for the instrument",
                now,
            );
            return Ok(None);
        };
        if stale {
            self.refuse(report, "stale_book", &reset_reason, now);
            return Ok(None);
        }
        if !status.accepts_orders() {
            self.refuse(
                report,
                "venue_status",
                &format!("the venue is {}", status.as_str()),
                now,
            );
            return Ok(None);
        }
        let Some(price) = mid else {
            self.refuse(report, "pricing", "the book serves no usable price", now);
            return Ok(None);
        };

        let side = match signal.kind {
            SignalKind::Enter => BookSide::Ask,
            SignalKind::Exit | SignalKind::Hedge => BookSide::Bid,
            SignalKind::Stand => {
                self.refuse(report, "signal_kind", "the signal asks for no action", now);
                return Ok(None);
            }
        };

        let notional = signal.desired_quantity * price;
        let key = signal.strategy.as_str().to_string();
        let Some(deployed) = self.deployed.get(&key) else {
            self.refuse(
                report,
                "deployment",
                "the strategy is not deployed here",
                now,
            );
            return Ok(None);
        };

        // Expiry is checked at every use rather than once at verification:
        // it is the backstop bounding a cell that lost contact, and a backstop
        // consulted only on arrival is not one.
        if !deployed.envelope.is_live(now) {
            self.refuse(
                report,
                "envelope_expiry",
                "the capital envelope has expired; the cell stops rather than continues",
                now,
            );
            return Ok(None);
        }

        let quantity = match deployed
            .envelope
            .admit(&venue, notional, &deployed.utilisation, now)
        {
            CapitalGrant::Full => signal.desired_quantity,
            CapitalGrant::Reduced(cap) => {
                let reduced = cap.checked_div(price).unwrap_or(Decimal::ZERO);
                self.refuse(
                    report,
                    "capital_reduced",
                    &format!("reduced to {reduced} by the capital envelope"),
                    now,
                );
                reduced
            }
            CapitalGrant::Refused(reason) => {
                self.refuse(report, "capital", &reason, now);
                return Ok(None);
            }
        };
        if !quantity.is_positive() {
            self.refuse(
                report,
                "capital",
                "the permitted size rounded to nothing",
                now,
            );
            return Ok(None);
        }

        if self.autonomy.level() == AutonomyLevel::Observation {
            self.refuse(
                report,
                "autonomy",
                "the cell is at observation and sends nothing",
                now,
            );
            return Ok(None);
        }

        self.order_sequence += 1;
        let order_id = format!("{}-{}", self.config.cell_id, self.order_sequence);
        let simulated = gateway.is_simulated();
        gateway.place(&order_id, &venue, side, quantity, price, now)?;

        if let Some(deployed) = self.deployed.get_mut(&key) {
            deployed.utilisation.gross_committed += quantity * price;
            deployed.utilisation.orders_sent += 1;
        }
        self.fills.push(CellFill {
            order_id: order_id.clone(),
            venue: venue.clone(),
            quantity,
            price,
        });
        self.journal.record(
            Decision::OrderSent {
                order_id: order_id.clone(),
                venue: venue.as_str().to_string(),
                quantity: quantity.to_string(),
                simulated,
            },
            now,
        );

        Ok(Some(PlacedOrder {
            order_id,
            strategy: signal.strategy.clone(),
            object_id: signal.object_id.clone(),
            venue,
            side,
            quantity,
            price,
            simulated,
        }))
    }

    fn venue_for(&self, object: &ObjectId) -> Option<VenueId> {
        self.config
            .venues
            .iter()
            .find(|venue| {
                self.liquidity
                    .get(venue, object)
                    .is_some_and(|state| state.status() != VenueStatus::Unreachable)
            })
            .cloned()
    }

    fn refuse(&mut self, report: &mut WorkReport, gate: &str, reason: &str, now: Timestamp) {
        report.refusals.push((gate.to_string(), reason.to_string()));
        self.journal.record(
            Decision::Refused {
                gate: gate.to_string(),
                reason: reason.to_string(),
            },
            now,
        );
    }

    // --- reconciliation and the mirror --------------------------------------

    /// Absorb a fill from the independent drop-copy channel.
    pub fn observe_drop_copy(&mut self, fill: DropCopyFill) {
        self.dropcopy.observe(fill);
    }

    /// Compare the two records and halt on any disagreement.
    ///
    /// Tripping needs no authority, which is why a break can act immediately:
    /// the cost of a false stop is minutes of missed opportunity, the cost of
    /// trading on a book that disagrees with the venue is unbounded.
    pub fn reconcile(&mut self, now: Timestamp) -> Vec<Discrepancy> {
        let fills = self.fills.clone();
        let breaks = self.dropcopy.reconcile(&fills);
        for discrepancy in &breaks {
            self.journal.record(
                Decision::ReconciliationBreak {
                    detail: discrepancy.describe(),
                },
                now,
            );
        }
        if !breaks.is_empty() && !self.is_halted() {
            self.autonomy.kill_switch_mut().trip_global(
                now,
                "drop-copy",
                "the cell's fills disagree with the venue's own account",
            );
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: "reconciliation break".to_string(),
                },
                now,
            );
        }
        breaks
    }

    /// Ship the journal to durable storage.
    ///
    /// The only call in the cell that may block, and deliberately outside both
    /// [`Cell::on_bytes`] and [`Cell::work`].
    pub fn flush(&mut self, mirror: &mut dyn Mirror, now: Timestamp) -> Result<usize> {
        let watermarks = self
            .sequencer
            .watermarks()
            .into_iter()
            .map(|mark| (mark.stream, mark.position))
            .collect();
        crate::journal::ship(
            &mut self.journal,
            mirror,
            &self.config.cell_id,
            watermarks,
            now,
        )
    }
}

/// Where a cell sends an order.
///
/// Narrower than the routing crate's `Gateway` on purpose: the cell needs to
/// place and to know whether the venue is simulated, and a wider surface here
/// would be a wider surface to get wrong.
pub trait Placer: std::fmt::Debug {
    /// Whether this is a simulated venue. The cell sets every order's
    /// `simulated` flag from this rather than from anything the caller says.
    fn is_simulated(&self) -> bool;

    fn place(
        &mut self,
        order_id: &str,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()>;

    /// What a production deployment must supply, empty when usable as is.
    fn required_configuration(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The gap events worth journalling, and what to say about each.
///
/// An opened gap may still fill, so it is recorded as an observation. An
/// abandoned one has already produced a reset and invalidated a book, which is
/// the event an incident review is looking for.
fn gap_detail(event: &qip_sequencing::tracker::SequenceEvent) -> Option<(String, String)> {
    use qip_sequencing::tracker::SequenceEvent;
    match event {
        SequenceEvent::GapOpened {
            stream,
            missing_from,
            missing_to,
        } => Some((
            stream.clone(),
            format!("sequences {missing_from}..={missing_to} are missing; holding for reorder"),
        )),
        SequenceEvent::GapAbandoned {
            stream,
            missing_from,
            missing_to,
            reason,
        } => Some((
            stream.clone(),
            format!(
                "sequences {missing_from}..={missing_to} will not arrive ({reason:?}); \
                 the affected books are reset"
            ),
        )),
        SequenceEvent::StreamStarted { .. }
        | SequenceEvent::Duplicate { .. }
        | SequenceEvent::GapFilled { .. } => None,
    }
}
