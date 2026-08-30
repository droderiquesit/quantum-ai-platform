//! Chaos: faults drawn at random, and the invariants that hold anyway.
//!
//! `stress.rs` breaks one named thing at a time and asks what the platform
//! does about it. This file gives up on naming the failure. It draws faults
//! from a seeded stream and injects them at points nobody chose — halts on top
//! of halts, a book invalidated mid-decision, a grant that expires between two
//! commitments, a drop copy that disagrees while an operator is lifting an
//! earlier stop — and after *every* step asserts the same five properties.
//!
//! The point of randomising is that a scenario a person writes is a scenario a
//! person thought of. The interleavings that break real systems are the ones
//! nobody would have written down: not "the kill switch stops orders" but "the
//! kill switch stops orders on the step after a reconciliation break tripped
//! it while the book was already stale and the grant had two seconds left".
//!
//! The five invariants, each chosen because its failure is silent:
//!
//! 1. **A tripped kill switch stops orders.** Every one, at every entry point,
//!    for as long as it is tripped, and the refusal is marked as a safety
//!    control so nothing retries it.
//! 2. **No order is ever priced off a stale book.** A stale book is not a
//!    thinner market, it is an unknown one, and the last good price is the
//!    most convincing wrong number a system can hold.
//! 3. **The records verify.** The cell's journal chain and the platform's
//!    event log chain, after every fault, not merely at the end of a clean
//!    run. A chain that only verifies on a good day is not evidence.
//! 4. **Capital never exceeds its envelope.** Whatever sequence of admissions,
//!    expiries and reductions the stream produces, the committed total stays
//!    inside the gross limit somebody signed.
//! 5. **The platform can always say why it did nothing.** Every stage of every
//!    cycle carries a reason, and a cell that placed no order recorded a
//!    refusal naming the gate. Silence is the failure mode that makes the
//!    other four unfalsifiable.
//!
//! **Reproducing a failure.** Everything here is seeded: no wall clock, no
//! ambient RNG, no iteration over a hash map. Each run prints its seed before
//! it starts and every assertion message carries the seed and the step, so a
//! failure in CI is replayed by running the same binary — or, to isolate one,
//! by putting the reported seed into [`SEEDS`] on its own.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::{CapitalEnvelope, CapitalGrant, Utilisation};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, OrderId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, dec};
use qip_edge::cell::{Cell, CellConfig, Placer};
use qip_edge::dropcopy::DropCopyFill;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::OperatorIdentity;
use std::sync::Arc;

// --- the run --------------------------------------------------------------

/// The seeds this suite runs. Fixed, so the suite is a regression test rather
/// than a lottery that fails once a fortnight on a machine nobody can borrow.
///
/// Add a seed here to keep a failure that was found; do not replace one,
/// because a seed that once found something is the cheapest test there is.
const SEEDS: [u64; 6] = [
    0x5EED_0001,
    0x5EED_0002,
    0xC0FF_EE01,
    0xDEAD_BEEF,
    0x1234_5678,
    0xF00D_F00D,
];

/// Faults per run. Long enough for halts, expiries and invalidations to land
/// on top of each other rather than in tidy sequence.
const STEPS: usize = 200;

const CELL: &str = "chaos-1";
const VENUE: &str = "XLON";
const ENVELOPE_KEY: &[u8] = b"a-chaos-suite-envelope-signing-key";

/// The gross limit every envelope in this suite is signed for.
///
/// Held as a constant rather than read back off the envelope, so invariant 4
/// is checked against the number a human approved rather than against whatever
/// the object under test happens to report.
const GROSS_LIMIT: &str = "1000000";
const ORDER_LIMIT: &str = "100000";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("a decimal literal in a fixture")
}

/// The instruments this run trades. Three, so a scoped halt can be observed
/// to stop one thing and not the others.
const SYMBOLS: [&str; 3] = ["ACME", "BOREAS", "CERES"];

// --- the faults -------------------------------------------------------------

/// One thing that can go wrong, drawn from the stream.
///
/// Every variant is something a real deployment does: an operator stops the
/// platform, a health check stops one region, a feed dies, capital is spent,
/// a grant ages out, the venue's own record disagrees with ours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    HaltEverything,
    HaltOneScope,
    LiftTheGlobalHalt,
    LiftOneScope,
    LoseABook,
    ResynchroniseABook,
    CommitCapital,
    AgeTheGrant,
    DisagreeOnADropCopy,
    RunACycle,
}

impl Fault {
    const ALL: [Self; 10] = [
        Self::HaltEverything,
        Self::HaltOneScope,
        Self::LiftTheGlobalHalt,
        Self::LiftOneScope,
        Self::LoseABook,
        Self::ResynchroniseABook,
        Self::CommitCapital,
        Self::AgeTheGrant,
        Self::DisagreeOnADropCopy,
        Self::RunACycle,
    ];

    fn draw(rng: &mut Xoshiro256) -> Self {
        Self::ALL[rng.below(Self::ALL.len() as u64) as usize]
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::HaltEverything => "halt everything",
            Self::HaltOneScope => "halt one scope",
            Self::LiftTheGlobalHalt => "lift the global halt",
            Self::LiftOneScope => "lift one scope",
            Self::LoseABook => "lose a book",
            Self::ResynchroniseABook => "resynchronise a book",
            Self::CommitCapital => "commit capital",
            Self::AgeTheGrant => "age the grant",
            Self::DisagreeOnADropCopy => "disagree on a drop copy",
            Self::RunACycle => "run a cycle",
        }
    }
}

// --- fixtures ---------------------------------------------------------------

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in SYMBOLS {
        universe
            .insert(
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                    .venue("XNYS")
                    .sector(Sector::InformationTechnology)
                    .price(dec!("100"))
                    .provenance(Provenance::synthetic("chaos", start()))
                    .build(start())
                    .expect("valid instrument"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("chaos")
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        )
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.25 },
            )
            .with_rationale("no single name may dominate the book"),
        )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// A two-sided book, built from messages because there is no setter that
/// bypasses the feed.
fn book(symbol: &str, at: Timestamp) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(symbol), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in [
        (BookSide::Bid, "99", "500"),
        (BookSide::Bid, "98", "800"),
        (BookSide::Ask, "101", "400"),
        (BookSide::Ask, "102", "900"),
    ]
    .iter()
    .enumerate()
    {
        state.apply(&MarketMessage::new(
            object(symbol),
            Origin::new(venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side: *side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            at,
            at,
        ))?;
    }
    Ok(state)
}

fn signed_envelope(expires_at: Timestamp) -> Result<CapitalEnvelope> {
    let terms = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("chaos-strategy"),
            CELL,
            d(GROSS_LIMIT),
            d(ORDER_LIMIT),
            dec!("50000"),
            vec![venue()],
            start(),
            expires_at,
            "alice@example.com",
            signature,
        )
    };
    let unsigned = terms("unsigned")?;
    terms(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))
}

fn cell() -> Result<Cell> {
    Cell::new(
        CellConfig::new(CELL, "europe-west2").with_venue(venue()),
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
}

/// A gateway that would accept anything. Present so that "no order was sent"
/// is a statement about the gates rather than about the venue being shut.
#[derive(Debug, Default)]
struct WillingGateway {
    placed: usize,
}

impl Placer for WillingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed += 1;
        Ok(())
    }
}

/// Everything one chaotic run carries between steps.
#[derive(Debug)]
struct Run {
    seed: u64,
    step: usize,
    now: Timestamp,
    platform: Platform,
    cell: Cell,
    gateway: WillingGateway,
    envelope: VerifiedEnvelope,
    /// What the run has committed against the grant, as the cell would hold it.
    utilisation: Utilisation,
    /// Which instruments the run has invalidated the book for.
    stale: [bool; SYMBOLS.len()],
    /// Scopes halted by this run, so lifting one is a real operation rather
    /// than a no-op that would make invariant 1 vacuous.
    halted_scopes: Vec<String>,
    /// Counters, printed at the end so a run that exercised nothing is visible
    /// as such rather than passing quietly.
    orders_refused: usize,
    cycles_run: usize,
    capital_refused: usize,
}

impl Run {
    fn new(seed: u64) -> Result<Self> {
        let mut cell = cell()?;
        for symbol in SYMBOLS {
            cell.track(book(symbol, start())?);
        }
        Ok(Self {
            seed,
            step: 0,
            now: start(),
            platform: platform()?,
            cell,
            gateway: WillingGateway::default(),
            envelope: VerifiedEnvelope::verify(
                signed_envelope(start().saturating_add(Duration::from_hours(1)))?,
                ENVELOPE_KEY,
                CELL,
                start(),
            )?,
            utilisation: Utilisation::default(),
            stale: [false; SYMBOLS.len()],
            halted_scopes: Vec::new(),
            orders_refused: 0,
            cycles_run: 0,
            capital_refused: 0,
        })
    }

    /// The prefix every assertion message carries, so a failure names the run
    /// that produced it and nothing else has to be reconstructed.
    fn at(&self, fault: Fault) -> String {
        format!(
            "seed 0x{:X} step {} ({})",
            self.seed,
            self.step,
            fault.as_str()
        )
    }

    fn inject(&mut self, fault: Fault, rng: &mut Xoshiro256) -> Result<()> {
        let index = rng.below(SYMBOLS.len() as u64) as usize;
        let symbol = SYMBOLS[index];
        match fault {
            Fault::HaltEverything => {
                self.platform.autonomy_mut().kill_switch_mut().trip_global(
                    self.now,
                    "chaos",
                    "a fault injector stopped the platform",
                );
                self.cell.autonomy_mut().kill_switch_mut().trip_global(
                    self.now,
                    "chaos",
                    "a fault injector stopped the cell",
                );
            }
            Fault::HaltOneScope => {
                let scope = format!("scope:{symbol}");
                self.platform.autonomy_mut().kill_switch_mut().trip_scope(
                    scope.clone(),
                    self.now,
                    "chaos",
                    "one scope was stopped",
                );
                if !self.halted_scopes.contains(&scope) {
                    self.halted_scopes.push(scope);
                }
            }
            Fault::LiftTheGlobalHalt => {
                // Lifting needs an operator with a credential minted now: the
                // controller refuses a stale one, which is the asymmetry the
                // whole kill switch is built around.
                let operator = OperatorIdentity::verified("ops@example.com", "webauthn", self.now);
                self.platform
                    .autonomy_mut()
                    .kill_switch_mut()
                    .clear_global(&operator, self.now)?;
                self.cell
                    .autonomy_mut()
                    .kill_switch_mut()
                    .clear_global(&operator, self.now)?;
            }
            Fault::LiftOneScope => {
                if let Some(scope) = self.halted_scopes.pop() {
                    let operator =
                        OperatorIdentity::verified("ops@example.com", "webauthn", self.now);
                    self.platform
                        .autonomy_mut()
                        .kill_switch_mut()
                        .clear_scope(&scope, &operator, self.now)?;
                }
            }
            Fault::LoseABook => {
                let mut state = book(symbol, self.now)?;
                state.reset("the feed stopped and the book cannot be confirmed");
                self.cell.track(state);
                self.stale[index] = true;
            }
            Fault::ResynchroniseABook => {
                self.cell.track(book(symbol, self.now)?);
                self.stale[index] = false;
            }
            Fault::CommitCapital => {
                // Sizes that straddle the order limit, so the stream produces
                // full grants, reductions and refusals rather than only one.
                let notional = d(&format!("{}", 10_000 + rng.below(200_000)));
                match self
                    .envelope
                    .admit(&venue(), notional, &self.utilisation, self.now)
                {
                    CapitalGrant::Full => self.utilisation.gross_committed += notional,
                    CapitalGrant::Reduced(cap) => {
                        assert!(
                            cap <= notional,
                            "{}: a reduction was larger than the request",
                            self.at(fault)
                        );
                        self.utilisation.gross_committed += cap;
                    }
                    CapitalGrant::Refused(_) => self.capital_refused += 1,
                }
                self.utilisation.orders_sent += 1;
            }
            Fault::AgeTheGrant => {
                self.now = self
                    .now
                    .saturating_add(Duration::from_mins(1 + rng.below(30) as i64));
            }
            Fault::DisagreeOnADropCopy => {
                // A fill the cell never knew about: an unhedged position nobody
                // is watching, and the one break that must halt without asking.
                self.cell.observe_drop_copy(DropCopyFill {
                    order_id: format!("venue-only-{}", self.step),
                    venue: venue(),
                    quantity: dec!("100"),
                    price: dec!("100"),
                    at: self.now,
                });
                let breaks = self.cell.reconcile(self.now);
                assert!(
                    !breaks.is_empty(),
                    "{}: a fill the cell never sent reconciled cleanly",
                    self.at(fault)
                );
                assert!(
                    self.cell.is_halted(),
                    "{}: a reconciliation break did not stop the cell",
                    self.at(fault)
                );
            }
            Fault::RunACycle => {
                let report = self.platform.run_cycle(self.now);
                self.cycles_run += 1;
                assert!(
                    report.traversed_every_stage(),
                    "{}: a stage stopped running:\n{}",
                    self.at(fault),
                    report.summarise()
                );
                // Invariant 5, at the platform: every stage says something,
                // including the quiet ones. "Nothing happened" and "nothing was
                // attempted" are different, and a report that cannot tell them
                // apart is no use at three in the morning.
                for outcome in &report.stages {
                    assert!(
                        outcome.detail.trim().len() > 10,
                        "{}: the {} stage produced nothing legible: {:?}",
                        self.at(fault),
                        outcome.stage.as_str(),
                        outcome.detail
                    );
                }
                assert_eq!(
                    report.halted,
                    self.platform.autonomy().kill_switch().is_globally_tripped(),
                    "{}: the report disagreed with the kill switch about the halt",
                    self.at(fault)
                );
            }
        }
        // Time always moves forward, so no two steps share an instant and an
        // ordering question never has two answers.
        self.now = self.now.saturating_add(Duration::from_secs(1));
        Ok(())
    }

    /// The five invariants, checked after every fault.
    fn check(&mut self, fault: Fault) -> Result<()> {
        self.kill_switch_stops_orders(fault);
        self.no_price_comes_off_a_stale_book(fault);
        self.the_records_verify(fault)?;
        self.capital_stays_inside_its_envelope(fault);
        self.the_cell_can_say_why_it_was_quiet(fault)?;
        Ok(())
    }

    /// 1. A tripped kill switch stops orders — every one, every time.
    fn kill_switch_stops_orders(&mut self, fault: Fault) {
        let scope = self
            .halted_scopes
            .first()
            .cloned()
            .unwrap_or_else(|| "scope:ACME".to_string());
        let globally = self.platform.autonomy().kill_switch().is_globally_tripped();
        let halted = self.platform.autonomy().kill_switch().is_halted(&scope);
        let order = Order::new(
            OrderId::from_string(format!("ord-{}-{}", self.seed, self.step)),
            object("ACME"),
            Side::Buy,
            dec!("100"),
            OrderType::Market,
            dec!("100"),
            "prop-chaos",
            vec!["hyp-chaos".to_string()],
            scope.clone(),
            self.now,
        );
        let outcome = self.platform.submit_order(order, self.now);

        if halted {
            let refusal = outcome.expect_err(&format!(
                "{}: an order was accepted while {scope} was halted",
                self.at(fault)
            ));
            assert!(
                refusal.message().contains("halted"),
                "{}: the refusal did not say the platform was halted: {}",
                self.at(fault),
                refusal.message()
            );
            self.orders_refused += 1;
        }
        if globally {
            assert_eq!(
                self.platform.autonomy().level(),
                qip_risk_engine::autonomy::AutonomyLevel::Observation,
                "{}: a halted platform reported an acting autonomy level",
                self.at(fault)
            );
        }
        // Whatever else happened, nothing has reached a real venue. This
        // deployment cannot, and the assertion is what keeps that true rather
        // than merely intended.
        assert!(
            !self.platform.orders().has_live_fills(),
            "{}: a paper platform produced a live fill",
            self.at(fault)
        );
        assert!(
            self.platform.orders().reconciliation_breaks().is_empty(),
            "{}: the order book and the venue disagree: {:?}",
            self.at(fault),
            self.platform.orders().reconciliation_breaks()
        );
    }

    /// 2. No order is ever priced off a stale book.
    fn no_price_comes_off_a_stale_book(&self, fault: Fault) {
        for (index, symbol) in SYMBOLS.iter().enumerate() {
            let Some(state) = self.cell.liquidity().get(&venue(), &object(symbol)) else {
                panic!("{}: {symbol} stopped being tracked", self.at(fault));
            };
            if !self.stale[index] {
                assert!(
                    !state.is_stale(),
                    "{}: {symbol} went stale without a fault saying so",
                    self.at(fault)
                );
                continue;
            }
            assert!(
                state.is_stale(),
                "{}: {symbol} was invalidated and did not stay invalid",
                self.at(fault)
            );
            assert!(
                state.mid().is_none() && state.microprice().is_none(),
                "{}: {symbol} served a price from before it was invalidated",
                self.at(fault)
            );
            assert!(
                state.best_bid().is_none() && state.best_ask().is_none(),
                "{}: {symbol} served a touch it cannot stand behind",
                self.at(fault)
            );
            assert!(
                !state.prices_are_usable(),
                "{}: {symbol} called its stale prices usable",
                self.at(fault)
            );
            assert!(
                state.reset_reason().is_some(),
                "{}: {symbol} was invalidated without recording why",
                self.at(fault)
            );
        }
        // And the cell has sent nothing at all, so no order exists that could
        // have been priced off one.
        assert_eq!(
            self.gateway.placed,
            0,
            "{}: {} order(s) reached the gateway during a chaotic run",
            self.at(fault),
            self.gateway.placed
        );
    }

    /// 3. The records verify — after every fault, not only at the end.
    fn the_records_verify(&self, fault: Fault) -> Result<()> {
        self.cell.journal().verify().map_err(|sequence| {
            Error::invalid(format!(
                "{}: the cell's journal chain broke at sequence {sequence}",
                self.at(fault)
            ))
        })?;
        self.platform
            .event_log()
            .verify_chain()
            .map_err(|sequence| {
                Error::invalid(format!(
                    "{}: the event log chain broke at sequence {sequence}",
                    self.at(fault)
                ))
            })?;
        Ok(())
    }

    /// 4. Capital never exceeds the envelope somebody signed.
    fn capital_stays_inside_its_envelope(&self, fault: Fault) {
        assert!(
            self.utilisation.gross_committed <= d(GROSS_LIMIT),
            "{}: {} committed against a {GROSS_LIMIT} grant",
            self.at(fault),
            self.utilisation.gross_committed
        );
        // A grant past its expiry admits nothing, whatever headroom is left.
        // This is the only bound on a cell the centre can no longer reach.
        if !self.envelope.is_live(self.now) {
            assert!(
                self.envelope
                    .admit(&venue(), dec!("1"), &Utilisation::default(), self.now)
                    .is_refused(),
                "{}: an expired grant admitted capital",
                self.at(fault)
            );
        }
        // And a venue outside the grant is refused with the whole limit free,
        // because the two bounds are independent.
        assert!(
            self.envelope
                .admit(
                    &VenueId::new("XNYS"),
                    dec!("1"),
                    &Utilisation::default(),
                    self.now
                )
                .is_refused(),
            "{}: a venue outside the grant was admitted",
            self.at(fault)
        );
    }

    /// 5. The cell can always say why it did nothing.
    fn the_cell_can_say_why_it_was_quiet(&mut self, fault: Fault) -> Result<()> {
        let halted_before = self.cell.is_halted();
        let report = self.cell.work(self.now, &mut self.gateway)?;

        assert_eq!(
            report.halted,
            halted_before,
            "{}: the cell's report disagreed about whether it was halted",
            self.at(fault)
        );
        assert!(
            report.orders.is_empty(),
            "{}: the cell sent {} order(s)",
            self.at(fault),
            report.orders.len()
        );
        // The property: quiet is never unexplained. A pass with no strategy
        // deployed is the one legitimate exception, and this run always has
        // something deployed by the time it works, so there is none.
        assert!(
            !report.refusals.is_empty(),
            "{}: the cell placed nothing and recorded no reason",
            self.at(fault)
        );
        for (gate, reason) in &report.refusals {
            assert!(
                !gate.trim().is_empty() && reason.trim().len() > 3,
                "{}: a refusal named no gate or gave no reason: {gate:?} / {reason:?}",
                self.at(fault)
            );
        }
        if halted_before {
            assert!(
                report
                    .refusals
                    .iter()
                    .any(|(gate, _)| gate == "kill_switch"),
                "{}: a halted cell refused for some reason other than the halt: {:?}",
                self.at(fault),
                report.refusals
            );
        }
        // Every refusal is journalled, so the answer survives the process.
        assert!(
            self.cell
                .journal()
                .entries()
                .iter()
                .any(|entry| matches!(&entry.decision, Decision::Refused { .. })),
            "{}: refusals were reported and not recorded",
            self.at(fault)
        );
        Ok(())
    }
}

#[test]
fn randomly_injected_faults_never_break_the_five_invariants() -> Result<()> {
    for seed in SEEDS {
        // Printed before the run so a failure is replayable from the captured
        // output even when the panic itself is truncated.
        println!("chaos run: seed 0x{seed:X}, {STEPS} steps");

        let mut rng = Xoshiro256::seeded(seed);
        let mut run = Run::new(seed)?;

        // A strategy is deployed under a verified grant before the faults
        // start, so "the cell placed no order" is a statement about the gates
        // rather than about there being nothing to place.
        let (strategy, program) = trivial_strategy()?;
        run.cell.deploy(
            strategy,
            program,
            VerifiedEnvelope::verify(
                signed_envelope(start().saturating_add(Duration::from_hours(1)))?,
                ENVELOPE_KEY,
                CELL,
                start(),
            )?,
        )?;

        for step in 0..STEPS {
            run.step = step;
            let fault = Fault::draw(&mut rng);
            run.inject(fault, &mut rng)?;
            run.check(fault)?;
        }

        // A run that never halted anything, never spent anything and never ran
        // a cycle would pass every invariant and prove nothing. Assert that the
        // stream actually exercised the platform, and print what it did.
        println!(
            "  seed 0x{seed:X}: {} order refusal(s), {} cycle(s), {} capital refusal(s), \
             {} halt(s) recorded, {} journal entries",
            run.orders_refused,
            run.cycles_run,
            run.capital_refused,
            run.platform.autonomy().kill_switch().history().len(),
            run.cell.journal().len(),
        );
        assert!(
            run.orders_refused > 0,
            "seed 0x{seed:X} never halted the platform, so invariant 1 was never tested"
        );
        assert!(
            run.cycles_run > 0,
            "seed 0x{seed:X} never ran a cycle, so invariant 5 was never tested at the platform"
        );
        assert!(
            run.capital_refused > 0,
            "seed 0x{seed:X} never exhausted or expired its grant, so invariant 4 was never tested"
        );
        assert!(
            run.stale.iter().any(|stale| *stale) || run.cell.journal().len() > STEPS,
            "seed 0x{seed:X} never invalidated a book, so invariant 2 was never tested"
        );
    }
    Ok(())
}

/// A strategy that compiles and reads one feature.
///
/// What it computes does not matter here: the chaos run is about the gates in
/// front of it, and the cheapest well-typed program is the honest fixture.
fn trivial_strategy() -> Result<(
    qip_strategy::compile::CompiledStrategy,
    qip_strategy::program::Program,
)> {
    use qip_contracts::FeatureKey;
    use qip_contracts::signal::SignalKind;
    use qip_strategy::catalogue::FeatureCatalogue;
    use qip_strategy::compile::StrategyCompiler;
    use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};

    let subject = object("ACME");
    let pressure = FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;

    let spec = StrategySpec::new(
        StrategyId::new("chaos-strategy"),
        subject,
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.6),
        200,
    ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}
