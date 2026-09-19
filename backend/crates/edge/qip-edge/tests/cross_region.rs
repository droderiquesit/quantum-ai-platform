//! Blueprint §31.1's cross-region solve and §33.1's path-3 extension, as the
//! cell actually runs them.
//!
//! # What was blocking rows 3 to 6, and what these tests hold
//!
//! ADR 0068 landed §30.2's path router with a caller, and rows 3 to 6 stayed
//! out of reach for one reason: `Cell::install_arbitrage` built the router's
//! region map by putting *every* venue the cell may trade in the cell's own
//! region, so a transfer was always a transport edge and a cross-region cycle
//! could not be composed at all. These tests are about that seam being open
//! and about what now stands behind it — not about the vocabulary, which
//! `qip-routing`'s own tests hold.
//!
//! Three properties, each asserted separately because each fails separately:
//!
//! * a cell told that one of its venues is abroad composes **mirror edges**
//!   and is assigned §30.2's **path 3**, which no cell could reach before;
//! * the two gates behind it refuse under **different** names —
//!   `path_router` when the platform cannot say how it would execute the
//!   cycle at all, `path_extension` when it can and §31.1's conditions do not
//!   hold — because the first is a configuration fault and the second is a
//!   market or inventory state, and an operator reading one series for both
//!   cannot tell them apart;
//! * none of it sends anything. Every refusal test asserts the gateway saw
//!   nothing, and the one cycle that is assigned a path is still stopped by
//!   the extension.
//!
//! # The honest bound, asserted rather than left as prose
//!
//! A cell that holds no inventory cannot complete a cross-region cycle, and
//! that is §31.1 working rather than §31.1 missing. Every closed mirror cycle
//! has exactly one object this region acquires and one it disposes of, so one
//! of the two legs is always a local **sell** — and a region holding nothing
//! is never above its target, so its band never permits one. The blueprint
//! says the same thing in its SETUP line: *"hold asset X in BOTH regions"*.
//! `a_region_holding_nothing_cannot_sell_the_leg_it_would_have_to_sell` is
//! that bound, named so nobody reads the refusal as a defect.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy, VenueFacts,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, EpisodicDigest, InventoryTargets, PolicyPayload, Slot,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{
    Cell, CellConfig, GATE_CENTRE_DARK_REGION, GATE_DARK_REGION, GATE_PATH_EXTENSION,
    GATE_PATH_ROUTER, Placer, WorkReport,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::mirror::{MirrorArrangement, MirroredInstrument};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::region::RegionOutlook;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use qip_routing::path::ExecutionPath;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
/// The local venue.
const VENUE: &str = "CX";
/// The second venue. Unlike `tests/path_assignment.rs`, this one is placed in
/// **another region**, which is the whole subject of this file.
const VENUE_TWO: &str = "DX";
const REGION_TWO: &str = "us-east1";
/// A third venue, in the cell's **own** region, that no edge of the cycle
/// touches. §30.2's row 4 is "one side lacks inventory, hedge available
/// locally", and a hedge in the book the cycle is already trading is the
/// local leg not being done rather than cover for it — so the row needs a
/// venue at home that the cycle does not use, and this is it.
const VENUE_HEDGE: &str = "EX";
const DESK: &str = "arb-desk";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";

/// See `tests/path_assignment.rs` for why a transfer edge declares
/// observations at all: the net-edge calculator haircuts a cycle by its
/// fewest-observed edge, and a transfer's rate is one by definition.
const TRANSFER_OBSERVATIONS: u32 = 512;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn venue_two() -> VenueId {
    VenueId::new(VENUE_TWO)
}

fn venue_hedge() -> VenueId {
    VenueId::new(VENUE_HEDGE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn book_at(at: &VenueId, market: &str, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(market), at.clone(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(market),
            Origin::new(at.clone(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

/// BTC is ten per cent dearer at the foreign venue, and the local cell also
/// holds a book for the cash leg — which it needs, because §31.1 compares the
/// region's *own* price against the distributed reference and a mirrored
/// instrument with no local book has no price to compare.
fn books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book_at(&venue(), "BTCUSDT", ("59999", "10"), ("60000", "10"))?,
        book_at(&venue_two(), "BTCUSDT", ("66000", "10"), ("66001", "10"))?,
        book_at(
            &venue(),
            "USDTUSD",
            ("0.9999", "1000000"),
            ("1.0001", "1000000"),
        )?,
    ])
}

/// Buy BTC locally, move it abroad, sell it there, move the cash home. Both
/// transfers cross a region boundary, so both are mirror edges — which is not
/// a property of this graph but of the region map the cell supplies.
fn cross_region_graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    for at in [venue(), venue_two()] {
        graph.register_venue(
            at,
            VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
        );
    }
    graph.add_trade(
        Node::new(object("USDT"), venue()),
        Node::new(object("BTC"), venue()),
        Decimal::ONE,
        Decimal::ZERO,
        object("BTCUSDT"),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object("BTC"),
        venue(),
        venue_two(),
        Decimal::ZERO,
        t(1),
        TRANSFER_OBSERVATIONS,
    )?;
    graph.add_trade(
        Node::new(object("BTC"), venue_two()),
        Node::new(object("USDT"), venue_two()),
        Decimal::ONE,
        Decimal::ZERO,
        object("BTCUSDT"),
        BookSide::Bid,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object("USDT"),
        venue_two(),
        venue(),
        Decimal::ZERO,
        t(1),
        TRANSFER_OBSERVATIONS,
    )?;
    Ok(graph)
}

fn sizes() -> SizePolicy {
    SizePolicy::uniform(d("10000")).with(object("BTC"), d("0.16"))
}

fn scanner() -> OpportunityScanner {
    OpportunityScanner::new(
        SearchSettings {
            max_cycle_edges: 4,
            ..SearchSettings::default()
        },
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("500000")),
    )
}

fn signed_envelope() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(DESK),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue(), venue_two()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

fn desk() -> Result<ArbitrageDesk> {
    ArbitrageDesk::new(
        StrategyId::new(DESK),
        scanner(),
        cross_region_graph()?,
        sizes(),
        signed_envelope()?,
        8,
        Duration::from_secs(30),
    )
}

/// What the centre distributes for §31.1: a target and a reference price per
/// mirrored instrument. `None` is the platform as it stands — the tenth slot
/// has no producer, which the kernel's whitelist module argues at length —
/// and is the input the router refusal below is driven with.
struct Distributed {
    btc_target: &'static str,
    btc_reference: &'static str,
    usdt_target: &'static str,
    usdt_reference: &'static str,
    /// Whether the centre named a target for the **cash** leg at all.
    ///
    /// False is §30.2's row 4 precondition and not a broken fixture: a
    /// mirror is established only where the operator's band and the centre's
    /// target both exist, so withholding one target leaves one of the
    /// cycle's two mirror edges unestablished, which is exactly "one side
    /// lacks inventory". Withholding the *reference* instead would not do —
    /// that is §33.1's path-3 input and row 4 never reads it.
    publish_usdt_target: bool,
}

impl Distributed {
    /// A target this region is below on BTC, and a reference above the local
    /// BTC mid so §31.1 indicates a local buy — which is the direction the
    /// cycle takes.
    const fn workable() -> Self {
        Self {
            btc_target: "10",
            btc_reference: "60500",
            usdt_target: "0",
            usdt_reference: "0.99",
            publish_usdt_target: true,
        }
    }

    /// The same distribution with the cash leg's target withheld, which is
    /// what makes §30.2's row 3 ineligible and row 4 the only row left.
    const fn without_the_cash_target() -> Self {
        Self {
            btc_target: "10",
            btc_reference: "60500",
            usdt_target: "0",
            usdt_reference: "0.99",
            publish_usdt_target: false,
        }
    }

    fn slot(&self, produced_at: Timestamp) -> Slot<InventoryTargets> {
        let mut targets = BTreeMap::from([("BTC".to_string(), d(self.btc_target))]);
        if self.publish_usdt_target {
            targets.insert("USDT".to_string(), d(self.usdt_target));
        }
        Slot::produced(
            InventoryTargets {
                targets,
                reference_prices: BTreeMap::from([
                    ("BTC".to_string(), d(self.btc_reference)),
                    ("USDT".to_string(), d(self.usdt_reference)),
                ]),
            },
            produced_at,
        )
    }
}

/// A payload whose capability slots are fresh, so the sizing multiplier is
/// one and the desk scans rather than refusing to.
///
/// `targets` carries §31.1's tenth slot and `targets_produced_at` is when the
/// centre produced it, which is deliberately separate from `issued_at`: the
/// §33.1 extension measures the reference's window from the instant the fact
/// was produced, not from when the payload shipped.
fn policy(
    issued_at: Timestamp,
    targets: Option<(&Distributed, Timestamp)>,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, issued_at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        issued_at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        issued_at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        issued_at,
    );
    if let Some((distributed, produced_at)) = targets {
        payload.inventory_targets = distributed.slot(produced_at);
    }
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

/// The operator's half of §31.1: the band widths and the dislocation
/// threshold, neither of which arrives on the wire.
fn arrangement() -> Result<MirrorArrangement> {
    MirrorArrangement::new()
        .with_instrument(
            object("BTC"),
            MirroredInstrument::new(d("1"), d("20"), object("BTCUSDT"), d("100"))?,
        )
        .with_instrument(
            object("USDT"),
            MirroredInstrument::new(d("1000"), d("50000"), object("USDTUSD"), d("0.005"))?,
        )
        .with_round_trip(REGION_TWO, Duration::from_millis(28))
}

/// A cell whose second venue is abroad, unless `abroad` says otherwise.
fn cell_with(
    abroad: bool,
    mirror: Option<MirrorArrangement>,
    policy: VerifiedPolicy,
) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let config = if abroad {
        config.with_venue_in_region(venue_two(), REGION_TWO)?
    } else {
        config.with_venue(venue_two())
    };
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_arbitrage(desk()?)?;
    if let Some(mirror) = mirror {
        cell.install_mirror(mirror)?;
    }
    cell.apply_policy(policy, t(5))?;
    for state in books()? {
        cell.track(state);
    }
    Ok((cell, metrics))
}

#[derive(Debug, Default)]
struct RecordingGateway {
    placed: Vec<String>,
}

impl Placer for RecordingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push(order_id.to_string());
        Ok(())
    }
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

fn refusals_counted(metrics: &Metrics, gate: &str) -> u64 {
    metrics.snapshot().counter(
        names::EDGE_REFUSALS,
        &labels([("cell", CELL), ("region", REGION), ("gate", gate)]),
    )
}

/// The premise every test below rests on: the fixture really does hold one
/// profitable cycle, so an empty report is evidence about the change and not
/// about the books.
fn assert_one_opportunity(cell: &Cell) {
    let scanned = cell
        .arbitrage()
        .expect("the desk was installed")
        .scan(cell.liquidity(), t(10));
    assert_eq!(
        scanned.opportunities.len(),
        1,
        "the premise failed: the cross-region books hold {} cycles",
        scanned.opportunities.len()
    );
}

#[test]
fn a_cell_told_one_of_its_venues_is_abroad_composes_mirror_edges_and_is_assigned_path_three()
-> Result<()> {
    // The row this lane exists to open. Before §31.1 the region map put
    // every venue the cell may trade in the cell's own region, so this exact
    // cycle composed two *transport* edges and was assigned path 2 —
    // latency-equalised parallel dispatch, whose whole premise is one
    // process — for two legs on opposite sides of an ocean. The only thing
    // that differs between this test and the one below it is which region
    // the second venue is placed in.
    let (mut cell, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    assert_eq!(
        report.paths.len(),
        1,
        "a cross-region cycle was assigned no path: {:?}",
        report.refusals
    );
    let routed = &report.paths[0];
    assert_eq!(routed.path(), ExecutionPath::MirroredInventory);
    assert_eq!(routed.assignment.assigned().number(), 3);
    // The half that carries the meaning: path 2 was not merely out-ranked,
    // it was never eligible, because the composition holds no transport edge
    // at all.
    assert!(
        !routed
            .assignment
            .eligible()
            .contains(&ExecutionPath::CrossVenue),
        "a cycle across two regions was eligible for the single-region path"
    );
    assert!(
        routed.assignment.rationale().contains("across 2 region(s)"),
        "the rationale should say the cycle spans two regions: {}",
        routed.assignment.rationale()
    );
    // And the assignment reached the chain, so the decision is replayable
    // rather than only present in a report the caller happens to hold.
    let assigned: Vec<&Decision> = cell
        .journal()
        .entries()
        .iter()
        .map(|entry| &entry.decision)
        .filter(|decision| matches!(decision, Decision::CyclePathAssigned { .. }))
        .collect();
    assert_eq!(assigned.len(), 1, "the assignment did not reach the chain");
    match assigned[0] {
        Decision::CyclePathAssigned {
            path, path_name, ..
        } => {
            assert_eq!(*path, 3);
            assert_eq!(path_name, "mirrored_inventory");
        }
        other => panic!("the wrong decision was matched: {other:?}"),
    }
    Ok(())
}

#[test]
fn the_same_cell_with_both_venues_at_home_is_assigned_path_two_exactly_as_before() -> Result<()> {
    // The control for the test above, and the regression guard for every
    // cell that has no region annotation at all — which is every deployed
    // one. An empty `venue_regions` must leave `Cell::install_arbitrage`
    // building precisely the map ADR 0068 shipped.
    let (mut cell, _) = cell_with(
        false,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    let report = cell.work(t(10), &mut RecordingGateway::default())?;
    assert_one_opportunity(&cell);
    assert_eq!(
        report.paths.len(),
        1,
        "a single-region cycle was assigned no path: {:?}",
        report.refusals
    );
    assert_eq!(report.paths[0].path(), ExecutionPath::CrossVenue);
    assert!(
        refusals_under(&report, GATE_PATH_EXTENSION).is_empty(),
        "§33.1 names no row for path 2 and the extension refused anyway: {:?}",
        report.refusals
    );
    // §33.1's table starts at path 3, so the honest record for path 2 is
    // that the blueprint asked for nothing — which is a different fact from
    // a check that passed, and the chain carries which.
    let checked: Vec<(u8, bool)> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::PathExtensionChecked { path, has_row, .. } => Some((*path, *has_row)),
            _ => None,
        })
        .collect();
    assert_eq!(checked, vec![(2, false)]);
    Ok(())
}

#[test]
fn a_region_holding_nothing_cannot_sell_the_leg_it_would_have_to_sell() -> Result<()> {
    // §31.1's SETUP is "hold asset X in BOTH regions", and this is what that
    // line costs a region that holds nothing. Every closed mirror cycle has
    // one object this region acquires and one it disposes of; the cell buys
    // BTC locally, which its band permits because it is below target, and
    // spends USDT locally, which its band does not, because a region holding
    // zero against a target of zero is at target and §31.1's third row
    // permits either direction only at reduced size — a size nothing in this
    // gate can produce.
    //
    // The refusal is the control working. It is asserted here, under its own
    // name, so that nobody reading `path_extension` in a refusal series
    // takes it for a defect.
    let (mut cell, metrics) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    assert_eq!(
        cell.position(&venue(), &object("USDT")),
        Decimal::ZERO,
        "the premise is a cell that holds nothing"
    );
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    // The path was assigned — this is not the router refusing.
    assert_eq!(report.paths.len(), 1);
    assert_eq!(report.paths[0].path(), ExecutionPath::MirroredInventory);
    assert!(
        refusals_under(&report, GATE_PATH_ROUTER).is_empty(),
        "the router refused a cycle it had a row for: {:?}",
        report.refusals
    );

    let refused = refusals_under(&report, GATE_PATH_EXTENSION);
    assert_eq!(
        refused.len(),
        1,
        "the extension did not refuse: {refused:?}"
    );
    // The BTC leg is checked first and clears its band; the USDT leg is the
    // one that does not, and the refusal names it. Without the instrument in
    // the message an operator would have two bands and no way to tell which.
    assert!(
        refused[0].contains("the mirrored leg in USDT"),
        "the refusal should name the leg that failed: {}",
        refused[0]
    );
    assert!(
        refused[0].contains("at-target row permits either direction at reduced size"),
        "the refusal should name §31.1's row: {}",
        refused[0]
    );
    // The admitting half, and the reason this refusal is evidence of a gate
    // rather than of a gate that refuses everything. The BTC leg is checked
    // first — mirror edges are walked in traversal order — and it cleared
    // §33.1 in full: the reference was inside its window, the local price
    // against it indicated a buy, and the band permitted one. Only the cash
    // leg did not. A refusal naming BTC as well would mean nothing here had
    // passed anything.
    assert!(
        !refused[0].contains("the mirrored leg in BTC"),
        "the BTC leg should have cleared the extension: {}",
        refused[0]
    );
    // The consequence, which is the point: nothing of that cycle was sent.
    assert!(
        gateway.placed.is_empty(),
        "legs of a cycle the extension refused reached the venue: {:?}",
        gateway.placed
    );
    // Charted under its own gate and not the router's, because the two are
    // different findings about a cell.
    assert_eq!(refusals_counted(&metrics, GATE_PATH_EXTENSION), 1);
    assert_eq!(refusals_counted(&metrics, GATE_PATH_ROUTER), 0);
    Ok(())
}

#[test]
fn a_cross_region_cycle_at_a_cell_with_no_mirror_arrangement_is_refused_whole_by_the_router()
-> Result<()> {
    // Fail closed at the first of the two gates: a cell that was told a
    // venue is abroad and given no §31.1 discipline cannot say what
    // inventory it is meant to hold, so it cannot supply a mirror fact and
    // §30.2 assigns nothing. The alternative — treating a missing
    // arrangement as no constraint — would route a cross-ocean cycle under
    // whatever row happened to be left.
    let (mut cell, metrics) = cell_with(
        true,
        None,
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    assert!(
        report.paths.is_empty(),
        "a cycle with no mirror discipline was assigned a path: {:?}",
        report.paths
    );
    let refused = refusals_under(&report, GATE_PATH_ROUTER);
    assert_eq!(refused.len(), 1, "the router did not refuse: {refused:?}");
    assert!(
        refused[0].contains("no §31.1 mirror arrangement is installed"),
        "the refusal should name what is missing: {}",
        refused[0]
    );
    assert!(gateway.placed.is_empty());
    assert_eq!(refusals_counted(&metrics, GATE_PATH_ROUTER), 1);
    assert_eq!(
        refusals_counted(&metrics, GATE_PATH_EXTENSION),
        0,
        "a cycle the router refused was also charted against the extension"
    );
    Ok(())
}

#[test]
fn a_mirror_the_centre_named_no_inventory_target_for_is_not_an_established_mirror() -> Result<()> {
    // The tenth policy slot has no producer in this platform — the kernel's
    // whitelist module argues why at length — so this is the state every
    // deployed cell would be in today. §30.2's row 3 turns on the mirror
    // being *established*, which is the centre's target plus the operator's
    // band, and with the target absent no row of the table admits the cycle.
    //
    // It is refused rather than assigned a nearest row, and the message is
    // the table's own rather than a missing-fact message, because the
    // missing fact is upstream of the table.
    let (mut cell, _) = cell_with(true, Some(arrangement()?), policy(t(5), None)?)?;
    assert!(
        cell.inventory_targets().is_none(),
        "the premise is a payload with no tenth slot"
    );
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    assert!(report.paths.is_empty());
    let refused = refusals_under(&report, GATE_PATH_ROUTER);
    assert_eq!(refused.len(), 1, "the router did not refuse: {refused:?}");
    assert!(
        refused[0].contains("no execution path is eligible"),
        "the refusal should be the table's own: {}",
        refused[0]
    );
    assert!(gateway.placed.is_empty());
    Ok(())
}

#[test]
fn a_reference_the_centre_stopped_republishing_refuses_at_the_extension_and_not_at_the_router()
-> Result<()> {
    // §31.1: "A stale reference can cost one side's band, never both." The
    // window is the tenth slot's own time to live, measured from when the
    // centre produced the fact. `Cell::inventory_targets` deliberately does
    // not pre-filter on freshness, precisely so this arm can fire — a cell
    // that filtered first would leave §33.1's "reference inside TTL" a check
    // no input could reach, and would report a stale reference as a missing
    // band.
    let produced_at = t(5);
    let (mut cell, metrics) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), produced_at)))?,
    )?;
    // Sixty seconds is `PolicyItem::InventoryTargets::time_to_live`. One
    // second short of it the reference still gates; at it, it does not.
    let report = cell.work(t(64), &mut RecordingGateway::default())?;
    assert_one_opportunity(&cell);
    assert!(
        refusals_under(&report, GATE_PATH_EXTENSION)
            .iter()
            .all(|reason| !reason.contains("stopped republishing")),
        "a reference inside its window was read as stale: {:?}",
        report.refusals
    );

    let (mut cell, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), produced_at)))?,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(65), &mut gateway)?;
    assert_eq!(
        report.paths.len(),
        1,
        "the router refused a cycle it had a row for: {:?}",
        report.refusals
    );
    let refused = refusals_under(&report, GATE_PATH_EXTENSION);
    assert_eq!(
        refused.len(),
        1,
        "the extension did not refuse: {refused:?}"
    );
    assert!(
        refused[0].contains("stopped republishing"),
        "the refusal should name the stale reference: {}",
        refused[0]
    );
    assert!(
        refused[0].contains("the mirrored leg in BTC"),
        "the first leg checked is the one that should be named: {}",
        refused[0]
    );
    assert!(gateway.placed.is_empty());
    assert_eq!(refusals_counted(&metrics, GATE_PATH_ROUTER), 0);
    Ok(())
}

#[test]
fn a_region_whose_own_price_puts_it_on_the_other_side_of_the_reference_may_not_take_the_cycle()
-> Result<()> {
    // §31.1's construction, at the cell: the direction a region may take is
    // decided by its own price against the distributed reference, so two
    // regions can never be permitted the same side. Here the reference is
    // put *below* the local BTC mid, which permits this region only to sell
    // BTC — and the cycle buys it. Nothing about the inventory band changed
    // between this test and the one above; only the reference did.
    let distributed = Distributed {
        btc_reference: "50000",
        ..Distributed::workable()
    };
    let (mut cell, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&distributed, t(5))))?,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    assert_eq!(report.paths.len(), 1, "the router should still assign");
    let refused = refusals_under(&report, GATE_PATH_EXTENSION);
    assert_eq!(
        refused.len(),
        1,
        "the extension did not refuse: {refused:?}"
    );
    assert!(
        refused[0].contains("permits only sell"),
        "the refusal should name the direction the reference permits: {}",
        refused[0]
    );
    assert!(
        refused[0].contains("the mirrored leg in BTC"),
        "the refusal should name the leg: {}",
        refused[0]
    );
    assert!(gateway.placed.is_empty());
    Ok(())
}

#[test]
fn a_region_annotation_for_a_venue_the_cell_may_not_trade_is_refused_at_installation() -> Result<()>
{
    // The guard that keeps §31.1 from widening anything. A region annotation
    // says *where* a venue is and never that the cell may reach it, and this
    // is the runtime half of that: `CellConfig::with_venue_in_region` cannot
    // produce such an entry, but the field is `pub` and the builder is
    // skippable, so the check is not redundant with it.
    let mut config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_venue_in_region(venue_two(), REGION_TWO)?;
    // A *third* venue, named only by the annotation. The desk's own graph
    // touches only the two above, so the older guard —
    // `install_arbitrage` refusing a graph edge at a venue outside
    // `config.venues` — has nothing to say about it, and this check is the
    // only thing that does.
    config
        .venue_regions
        .insert("EX".to_string(), "ap-south1".to_string());
    // Premise: the venue really is absent from the list the cell may trade,
    // and the two the graph reaches really are present.
    assert!(!config.venues.contains(&VenueId::new("EX")));
    assert!(config.venues.contains(&venue()) && config.venues.contains(&venue_two()));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    let refusal = cell
        .install_arbitrage(desk()?)
        .expect_err("a region annotation cannot place a venue the cell may not trade");
    assert_eq!(refusal.code(), "denied");
    assert!(
        refusal
            .message()
            .contains("venue EX is placed in region ap-south1"),
        "the refusal should name the venue and the region: {}",
        refusal.message()
    );
    assert!(
        refusal.message().contains("is not one this cell may trade"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // The half that proves the check admits a good configuration: the same
    // cell without the third annotation installs.
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut good = Cell::new(
        CellConfig::new(CELL, REGION)
            .with_venue(venue())
            .with_venue_in_region(venue_two(), REGION_TWO)?,
        features,
    )?;
    assert!(good.install_arbitrage(desk()?).is_ok());
    Ok(())
}

#[test]
fn a_region_id_with_surrounding_whitespace_is_refused_rather_than_trimmed() -> Result<()> {
    // Two region ids differing by a space are two regions to a mirror edge,
    // so a value corrected here would be a configuration bug that survived
    // into every later pass — a venue placed in a region the router never
    // produces, and every cycle through it refused for a reason naming the
    // venue rather than the typo.
    let refusal = CellConfig::new(CELL, REGION)
        .with_venue_in_region(venue_two(), " us-east1")
        .expect_err("whitespace makes it a different region");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("surrounding whitespace"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // The half that proves it admits a good value.
    assert!(
        CellConfig::new(CELL, REGION)
            .with_venue_in_region(venue_two(), REGION_TWO)
            .is_ok()
    );
    Ok(())
}

#[test]
fn a_mirror_arrangement_naming_this_cells_own_region_is_refused_as_a_measurement_nothing_reads()
-> Result<()> {
    // A mirror edge is one asset in two regions and the router refuses one
    // whose ends share a region, so a round trip recorded to the cell's own
    // region could never be looked up. Refusing it names the configuration
    // error instead of leaving a number that reads as a measurement.
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(CellConfig::new(CELL, REGION).with_venue(venue()), features)?;
    let refusal = cell
        .install_mirror(
            MirrorArrangement::new()
                .with_instrument(
                    object("BTC"),
                    MirroredInstrument::new(d("1"), d("20"), object("BTCUSDT"), d("100"))?,
                )
                .with_round_trip(REGION, Duration::from_millis(28))?,
        )
        .expect_err("a round trip to one's own region reaches no mirror edge");
    assert_eq!(refusal.code(), "denied");
    assert!(
        refusal.message().contains("own region"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // And an arrangement naming another region installs, so the check is not
    // refusing everything.
    assert!(cell.install_mirror(arrangement()?).is_ok());
    assert!(
        cell.install_mirror(arrangement()?).is_err(),
        "a second arrangement would move a band under a cycle gated against the first"
    );
    Ok(())
}

#[test]
fn a_mirror_arrangement_naming_no_instrument_is_refused_at_installation() -> Result<()> {
    // An arrangement with no instrument gates nothing, and a cell that
    // installed one would look configured for §31.1 while refusing every
    // cross-region cycle for a missing band. The refusal names the
    // configuration rather than letting the first cycle report a market
    // problem that is not one.
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(CellConfig::new(CELL, REGION).with_venue(venue()), features)?;
    let empty = MirrorArrangement::new().with_round_trip(REGION_TWO, Duration::from_millis(28))?;
    // Premise: it really does carry the round trip, so this is not an
    // entirely empty value being refused for some other reason.
    assert!(empty.round_trip(REGION_TWO).is_ok());
    assert_eq!(empty.len(), 0);
    let refusal = cell
        .install_mirror(empty)
        .expect_err("an arrangement naming no instrument gates nothing");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("naming no instrument"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // The half that proves it admits a good value.
    assert!(cell.install_mirror(arrangement()?).is_ok());
    Ok(())
}

#[test]
fn a_cross_region_cycle_whose_remote_region_nobody_measured_a_round_trip_to_is_refused()
-> Result<()> {
    // `MirrorFacts::new` refuses a round trip of zero because it makes every
    // remote quote look like it outlasts the wire, and a default here would
    // be a number nobody measured sitting where §30.2's row 6 is decided.
    let arrangement = MirrorArrangement::new()
        .with_instrument(
            object("BTC"),
            MirroredInstrument::new(d("1"), d("20"), object("BTCUSDT"), d("100"))?,
        )
        .with_instrument(
            object("USDT"),
            MirroredInstrument::new(d("1000"), d("50000"), object("USDTUSD"), d("0.005"))?,
        )
        .with_round_trip("ap-south1", Duration::from_millis(120))?;
    let (mut cell, _) = cell_with(
        true,
        Some(arrangement),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    assert!(report.paths.is_empty());
    let refused = refusals_under(&report, GATE_PATH_ROUTER);
    assert_eq!(refused.len(), 1, "the router did not refuse: {refused:?}");
    assert!(
        refused[0].contains("no round trip to region us-east1"),
        "the refusal should name the unmeasured region: {}",
        refused[0]
    );
    assert!(gateway.placed.is_empty());
    Ok(())
}

#[test]
fn a_mirrored_instrument_with_no_local_book_is_refused_rather_than_priced_on_one_side() -> Result<()>
{
    // §31.1 compares this region's *own* price against the distributed
    // reference. A cell whose book for the mirrored instrument's market is
    // one-sided has no mid, and taking whichever side exists would compare a
    // touch against a reference struck on a mid and read every thin moment
    // as a dislocation.
    let (mut cell, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    // Replace the cash book with a bid-only one. The premise is that the
    // cycle is otherwise unchanged: the BTC books are untouched, so the scan
    // still finds it.
    let mut one_sided = VenueState::aggregated(object("USDTUSD"), venue(), VenueStatus::Open);
    one_sided.apply(&MarketMessage::new(
        object("USDTUSD"),
        Origin::new(venue(), "feed-a", 0, 9),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: d("0.9999"),
            quantity: d("1000000"),
            order_count: None,
        },
        t(2),
        t(2),
    ))?;
    cell.track(one_sided);
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    assert_eq!(report.paths.len(), 1, "the router should still assign");
    let refused = refusals_under(&report, GATE_PATH_EXTENSION);
    assert_eq!(
        refused.len(),
        1,
        "the extension did not refuse: {refused:?}"
    );
    assert!(
        refused[0].contains("is not two-sided"),
        "the refusal should name the one-sided book: {}",
        refused[0]
    );
    assert!(gateway.placed.is_empty());
    Ok(())
}

// --- §36.3: a region that has gone dark --------------------------------------

#[test]
fn a_cycle_whose_mirrored_leg_reaches_a_dark_region_is_suspended_under_its_own_gate() -> Result<()>
{
    // §36.3's node-crash and region-failure rows, in the column that applies
    // to every *other* region: "mirrors involving it suspend". The cell on
    // the other end of this mirror is not there to take its side, and a
    // region that keeps trading one side of a mirror whose other side is
    // nobody is left long in one region and hedged in neither.
    //
    // The premise is the test above it: this exact fixture is assigned path
    // 3 and reaches §33.1's extension. Asserted here rather than assumed,
    // because "no path was assigned" is also what a broken fixture looks
    // like.
    let (mut lit, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    let mut unused = RecordingGateway::default();
    let routed = lit.work(t(10), &mut unused)?;
    assert_eq!(
        routed.paths.len(),
        1,
        "the premise failed: this fixture is assigned no path at all: {:?}",
        routed.refusals
    );
    assert!(unused.placed.is_empty());

    let (mut cell, metrics) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    cell.apply_region_outlook(RegionOutlook::declared([REGION_TWO.to_string()])?, t(9));
    assert!(cell.is_region_dark(REGION_TWO));
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    let suspended = refusals_under(&report, GATE_DARK_REGION);
    assert_eq!(
        suspended.len(),
        1,
        "a mirror into a dark region was not suspended: {:?}",
        report.refusals
    );
    assert!(
        suspended[0].contains(REGION_TWO),
        "the refusal should name the region that went dark: {}",
        suspended[0]
    );
    assert_eq!(
        refusals_counted(&metrics, GATE_DARK_REGION),
        1,
        "the suspension was journaled and not counted"
    );
    // Under its own gate, and not under the router's: "the router had no row
    // for this cycle" and "the cell at the other end is not answering" send
    // an operator to two different places.
    assert!(
        refusals_under(&report, GATE_PATH_ROUTER).is_empty(),
        "a suspended mirror was charted as a routing failure: {:?}",
        report.refusals
    );
    assert!(
        report.paths.is_empty(),
        "a suspended cycle was still assigned a path"
    );
    assert!(gateway.placed.is_empty(), "a suspended mirror still sent");
    Ok(())
}

/// The same policy with slot 11 produced, naming `dark` as the regions the
/// centre has derived dark (ADR 0079).
fn policy_with_dark_regions(
    issued_at: Timestamp,
    targets: Option<(&Distributed, Timestamp)>,
    dark: &[&str],
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, issued_at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        issued_at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        issued_at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        issued_at,
    );
    if let Some((distributed, produced_at)) = targets {
        payload.inventory_targets = distributed.slot(produced_at);
    }
    payload.feasibility_constraints = Slot::produced(
        qip_contracts::policy::FeasibilityConstraints {
            minimum_order: BTreeMap::new(),
            fee_floor: BTreeMap::new(),
            tick: BTreeMap::new(),
            withdrawn_venues: std::collections::BTreeSet::new(),
            dark_regions: dark.iter().map(|region| (*region).to_string()).collect(),
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

#[test]
fn a_mirror_into_a_region_the_centre_derived_dark_is_refused_at_the_extension_under_the_centres_token()
-> Result<()> {
    // ADR 0079 decision five. The cell's own region wire says every peer is
    // lit — asserted, because that is the premise: this refusal comes from
    // the centre's derivation on slot 11 and from nothing this node read
    // locally. A region this cell holds no venue in suspends nothing here,
    // for the reason `dark_foreign_regions` gives: a name that costs this
    // cell nothing must not read as a suspended mirror.
    let (mut elsewhere, _) = cell_with(
        true,
        Some(arrangement()?),
        policy_with_dark_regions(
            t(5),
            Some((&Distributed::workable(), t(5))),
            &["ap-south-1"],
        )?,
    )?;
    let mut unused = RecordingGateway::default();
    let routed = elsewhere.work(t(10), &mut unused)?;
    assert_eq!(
        routed.paths.len(),
        1,
        "the premise failed: this fixture is assigned no path at all: {:?}",
        routed.refusals
    );
    assert!(
        !refusals_under(&routed, GATE_PATH_EXTENSION)
            .iter()
            .any(|reason| reason.contains(&format!("{GATE_CENTRE_DARK_REGION}:"))),
        "a dark region this cell holds no venue in suspended its mirror: {:?}",
        routed.refusals
    );

    let (mut cell, metrics) = cell_with(
        true,
        Some(arrangement()?),
        policy_with_dark_regions(t(5), Some((&Distributed::workable(), t(5))), &[REGION_TWO])?,
    )?;
    assert!(
        !cell.is_region_dark(REGION_TWO),
        "the premise failed: the cell's own wire reads the region dark, so this would be \
         `dark_mirror`'s refusal and not the centre's"
    );
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    // Charted under the extension's gate — the constant that seam already
    // passes, so the series gains no value — and the reason opens with the
    // centre's token so the journal tells the two dark findings apart. The
    // token is the property, not the count: this fixture's path-3 band
    // already refuses at the extension on its own facts, so a refusal under
    // the gate exists with or without the centre's derivation, and a test
    // that counted one would pass with the check deleted. A mutation found
    // exactly that.
    let refused = refusals_under(&report, GATE_PATH_EXTENSION);
    assert_eq!(
        refused.len(),
        1,
        "the premise failed: the mirrored leg was not refused at the extension at all: {:?}",
        report.refusals
    );
    assert!(
        refused[0].contains(&format!("{GATE_CENTRE_DARK_REGION}:")),
        "a mirror into a region the centre derived dark was not refused under the centre's \
         token; the extension refused it on its own facts instead: {}",
        refused[0]
    );
    assert!(
        refused[0].contains(REGION_TWO),
        "the refusal should name the region the centre derived dark: {}",
        refused[0]
    );
    assert_eq!(refusals_counted(&metrics, GATE_PATH_EXTENSION), 1);
    assert!(
        refusals_under(&report, GATE_DARK_REGION).is_empty(),
        "the centre's derivation was charted as this cell's own wire reading: {:?}",
        report.refusals
    );
    assert!(gateway.placed.is_empty(), "a suspended mirror still sent");
    Ok(())
}

#[test]
fn an_unreadable_region_wire_suspends_the_mirror_and_a_region_this_cell_never_reaches_does_not()
-> Result<()> {
    // Two halves of one property, because each fails separately. A wire that
    // cannot be read says nothing about which peer is alive, so the
    // fail-closed reading suspends every mirror — a reading that guessed
    // "nothing is dark" would let this cell take one side of a trade whose
    // other side may be gone. And a region this cell holds no venue in
    // suspends nothing here, because none of its mirror edges reach it: a
    // declaration that stopped an unrelated cell would make a regional
    // outage a platform outage, which is the opposite of isolation.
    let (mut blind, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    blind.apply_region_outlook(RegionOutlook::unreadable("the mount is gone")?, t(9));
    let mut blind_gateway = RecordingGateway::default();
    let blinded = blind.work(t(10), &mut blind_gateway)?;
    assert_one_opportunity(&blind);
    assert_eq!(
        refusals_under(&blinded, GATE_DARK_REGION).len(),
        1,
        "an unreadable wire did not suspend the mirror: {:?}",
        blinded.refusals
    );
    assert!(blind_gateway.placed.is_empty());

    let (mut elsewhere, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    elsewhere.apply_region_outlook(RegionOutlook::declared(["ap-south-1".to_string()])?, t(9));
    // Premise: the reading really is in force, so the cycle below is
    // unaffected because the region is unrelated and not because nothing was
    // applied.
    assert!(elsewhere.is_region_dark("ap-south-1"));
    assert!(!elsewhere.is_region_dark(REGION_TWO));
    let mut untouched = RecordingGateway::default();
    let report = elsewhere.work(t(10), &mut untouched)?;
    assert!(
        refusals_under(&report, GATE_DARK_REGION).is_empty(),
        "a region this cell has no venue in suspended its mirror: {:?}",
        report.refusals
    );
    assert_eq!(
        report.paths.len(),
        1,
        "the cycle should still be assigned its path: {:?}",
        report.refusals
    );
    Ok(())
}

#[test]
fn a_dark_region_leaves_a_cell_whose_venues_are_all_at_home_trading_exactly_as_before() -> Result<()>
{
    // §36.3's "everything else" column: others unaffected. The same
    // declaration that suspends a mirror must not touch a cell whose cycle
    // never leaves its own region — an isolation control that stops the
    // regions that were working is an outage it caused itself.
    let (mut cell, metrics) = cell_with(false, None, policy(t(5), None)?)?;
    cell.apply_region_outlook(RegionOutlook::declared([REGION_TWO.to_string()])?, t(9));
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_one_opportunity(&cell);

    assert_eq!(
        report.paths.len(),
        1,
        "a local cycle was not assigned a path while another region was dark: {:?}",
        report.refusals
    );
    assert_eq!(report.paths[0].assignment.assigned().number(), 2);
    assert!(
        refusals_under(&report, GATE_DARK_REGION).is_empty(),
        "a local cycle was suspended by another region's outage: {:?}",
        report.refusals
    );
    assert_eq!(refusals_counted(&metrics, GATE_DARK_REGION), 0);
    Ok(())
}

#[test]
fn a_change_in_what_the_cell_believes_about_its_peers_reaches_the_chain_once() -> Result<()> {
    // The suspension has to be replayable from the journal alone: "why did
    // this cell stop mirroring" has the same standing as "why did it trade".
    // Once, not once per poll — the node reads the wire every pass, and a
    // chain entry per pass would bury the moment it changed.
    let (mut cell, _) = cell_with(
        true,
        Some(arrangement()?),
        policy(t(5), Some((&Distributed::workable(), t(5))))?,
    )?;
    let outlook = RegionOutlook::declared([REGION_TWO.to_string()])?;
    cell.apply_region_outlook(outlook.clone(), t(9));
    cell.apply_region_outlook(outlook, t(10));
    let changes: Vec<&Decision> = cell
        .journal()
        .entries()
        .iter()
        .map(|entry| &entry.decision)
        .filter(|decision| matches!(decision, Decision::RegionOutlookChanged { .. }))
        .collect();
    assert_eq!(
        changes.len(),
        1,
        "a re-read of an unchanged wire wrote a second entry: {changes:?}"
    );
    match changes[0] {
        Decision::RegionOutlookChanged {
            source, regions, ..
        } => {
            assert_eq!(source, "declared");
            assert_eq!(regions, &vec![REGION_TWO.to_string()]);
        }
        other => panic!("the chain holds the wrong entry: {other:?}"),
    }

    // And a release is an event too, or a reader of the chain could never
    // tell when the mirror resumed.
    cell.apply_region_outlook(RegionOutlook::AllLit, t(11));
    let after: usize = cell
        .journal()
        .entries()
        .iter()
        .filter(|entry| matches!(entry.decision, Decision::RegionOutlookChanged { .. }))
        .count();
    assert_eq!(after, 2, "the release left no entry");
    Ok(())
}

// --- §30.2's row 4 and §33.1's path-4 hedge check ----------------------------
//
// `Cell::local_hedges_for` answers both halves of row 4 from one value: the
// router asks whether a hedge *exists* before it will assign path 4, and
// §33.1's extension asks whether that same hedge is *deep enough* before the
// cycle may go. Until it existed the cell passed a literal `false` for the
// hedge fact, so row 4 was unreachable from a cell and the path-4 arm of
// `qip_routing::extension::check` was a gate no cell input could put a fact
// in front of — the `MaxExpectedShortfall` shape, in the router.
//
// The tests below drive the whole seam through `Cell::work`, because that is
// where the one-value claim is decided: a value read twice, once for the
// router and once for the gate, would let a cycle be assigned on one reading
// of the books and gated on another, and only a test that runs both in one
// pass can see it.

/// A second home venue whose hedge book is deliberately shallower than
/// [`VENUE_HEDGE`]'s, so "the deepest local book is taken" is a claim about
/// a choice rather than about the only candidate there was.
const VENUE_HEDGE_TWO: &str = "FX";

fn venue_hedge_two() -> VenueId {
    VenueId::new(VENUE_HEDGE_TWO)
}

/// The size the scan plans this fixture's cycle at, and therefore the size
/// §33.1 measures a hedge against — `Cell::local_hedges_for` takes the first
/// plan step's quantity.
///
/// Asserted as a premise in every test below rather than trusted. "Deep
/// enough" and "too thin" are labels on a comparison against this number,
/// and if the fixture's size policy moved they would silently become labels
/// on nothing.
const FIRST_LEG: &str = "0.166666667";

/// A hedge book deeper than [`FIRST_LEG`], and one shallower than it. Both
/// are strings the book is built with, so the depth a test names is the
/// depth the cell sweeps.
const DEEP: &str = "10";
const THIN: &str = "0.1";
/// Shallower than [`THIN`] — the loser of the deepest-book choice.
const THINNER: &str = "0.05";

/// The local hedge books, at whatever depths the caller names.
///
/// Which side of each book matters and is the opposite of the local leg's
/// direction: the cell **buys** BTC locally, so the hedge sells and consumes
/// resting **bids**; it **sells** USDT locally, so that hedge buys and
/// consumes resting **asks**. `book_at` sets both sides, so a test cannot
/// pass by reading the wrong one — the sizes are equal and the assertion
/// that separates them is the venue named in the refusal.
fn hedge_books(btc_at_hedge: &str, btc_at_hedge_two: &str) -> Result<Vec<VenueState>> {
    Ok(vec![
        book_at(
            &venue_hedge(),
            "BTCUSDT",
            ("59990", btc_at_hedge),
            ("60010", btc_at_hedge),
        )?,
        book_at(
            &venue_hedge_two(),
            "BTCUSDT",
            ("59980", btc_at_hedge_two),
            ("60020", btc_at_hedge_two),
        )?,
        book_at(
            &venue_hedge(),
            "USDTUSD",
            ("0.9998", "1000000"),
            ("1.0002", "1000000"),
        )?,
    ])
}

/// A cell in exactly the state §30.2's row 4 describes: the same cross-region
/// cycle every test above uses, the centre's target for the **cash** leg
/// withheld so that mirror edge is not established and row 3 cannot be
/// eligible, and whatever local hedge books the caller supplies.
///
/// The two hedge venues are in the cell's own region — they carry no
/// `venue_regions` entry, and a venue absent from that map is at home — and
/// neither appears in the desk's graph, so nothing they hold can change what
/// the scan finds.
fn cell_for_row_four(hedge_books: Vec<VenueState>) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_venue_in_region(venue_two(), REGION_TWO)?
        .with_venue(venue_hedge())
        .with_venue(venue_hedge_two());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_arbitrage(desk()?)?;
    cell.install_mirror(arrangement()?)?;
    cell.apply_policy(
        policy(t(5), Some((&Distributed::without_the_cash_target(), t(5))))?,
        t(5),
    )?;
    for state in books()?.into_iter().chain(hedge_books) {
        cell.track(state);
    }
    Ok((cell, metrics))
}

/// The premises row 4 rests on, asserted together because each fails
/// separately and each would make the tests below assert nothing.
///
/// * the centre named a target for BTC and none for the cash leg, so one of
///   the two mirror edges is unestablished — which is what makes row 3
///   ineligible and row 4 the row under test;
/// * the books still hold exactly one cycle;
/// * the scan plans that cycle at [`FIRST_LEG`], which is the size every
///   hedge depth below is chosen against.
fn assert_row_four_premises(cell: &Cell) {
    let (targets, _) = cell
        .inventory_targets()
        .expect("the centre distributed a tenth slot");
    assert!(
        targets.targets.contains_key("BTC"),
        "the premise failed: the BTC mirror should be established"
    );
    assert!(
        !targets.targets.contains_key("USDT"),
        "the premise failed: the cash leg's target was supposed to be withheld"
    );
    let scanned = cell
        .arbitrage()
        .expect("the desk was installed")
        .scan(cell.liquidity(), t(10));
    assert_eq!(
        scanned.opportunities.len(),
        1,
        "the premise failed: the books hold {} cycles",
        scanned.opportunities.len()
    );
    let first = scanned.opportunities[0]
        .planned
        .plan
        .steps()
        .first()
        .expect("a planned cycle has a first leg")
        .quantity;
    assert_eq!(
        first,
        d(FIRST_LEG),
        "the premise failed: the hedge depths are chosen against {FIRST_LEG} and the scan plans \
         the first leg at {first}"
    );
}

#[test]
fn a_mirror_short_of_inventory_with_a_deep_local_hedge_is_assigned_path_four_and_clears_it()
-> Result<()> {
    // The row this finishes. §30.2's row 4 is "one side lacks inventory,
    // hedge available locally", and before `Cell::local_hedges_for` a cell
    // passed `false` for the hedge fact whatever its books held, so this
    // cycle was refused by the router with the table's own message and no
    // input existed that could have changed it.
    let (mut cell, metrics) = cell_for_row_four(hedge_books(DEEP, THINNER)?)?;
    let report = cell.work(t(10), &mut RecordingGateway::default())?;
    assert_row_four_premises(&cell);

    assert_eq!(
        report.paths.len(),
        1,
        "a cycle with a deep local hedge was assigned no path: {:?}",
        report.refusals
    );
    let routed = &report.paths[0];
    assert_eq!(routed.path(), ExecutionPath::HedgedBridging);
    assert_eq!(routed.assignment.assigned().number(), 4);
    // The half that carries the meaning. Row 3 is not merely out-ranked here,
    // it is ineligible — the cash leg's mirror is not established — so path 4
    // was assigned because the hedge existed and for no other reason.
    assert!(
        !routed
            .assignment
            .eligible()
            .contains(&ExecutionPath::MirroredInventory),
        "row 3 was eligible, so this cycle does not test row 4: {}",
        routed.assignment.rationale()
    );
    // And the gate the hedge exists to satisfy actually ran and held.
    assert!(
        refusals_under(&report, GATE_PATH_EXTENSION).is_empty(),
        "a hedge deeper than the first leg was refused at the extension: {:?}",
        report.refusals
    );
    assert_eq!(refusals_counted(&metrics, GATE_PATH_ROUTER), 0);
    assert_eq!(refusals_counted(&metrics, GATE_PATH_EXTENSION), 0);
    // §33.1's verdict reaches the chain naming the check it satisfied, so
    // "the hedge was there" is replayable from the log rather than only
    // present in a report the caller happens to hold.
    let checked: Vec<(u8, bool, String)> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::PathExtensionChecked {
                path,
                has_row,
                rationale,
                ..
            } => Some((*path, *has_row, rationale.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(checked.len(), 1, "the verdict did not reach the chain");
    assert_eq!(checked[0].0, 4);
    assert!(
        checked[0].1,
        "§33.1 has a row for path 4 and said it had none"
    );
    assert!(
        checked[0]
            .2
            .contains("hedge instrument available at depth before the first leg"),
        "the chained rationale should name §33.1's row-4 check: {}",
        checked[0].2
    );
    Ok(())
}

#[test]
fn a_local_hedge_too_thin_for_the_first_leg_refuses_at_the_extension_and_names_the_book()
-> Result<()> {
    // The refusal, and the reason one value serves both callers. The router
    // assigns path 4 because a hedge *exists*; the extension refuses because
    // that same hedge is not deep enough. Two reads of the books — one for
    // each question — would let the cycle be assigned against a book the gate
    // then measured from a different snapshot, and a cell whose order path
    // runs beside a live feed would take that race on every pass.
    let (mut cell, metrics) = cell_for_row_four(hedge_books(THIN, THINNER)?)?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_row_four_premises(&cell);

    // The path was assigned — this is the extension refusing, not the router.
    assert_eq!(
        report.paths.len(),
        1,
        "the router refused a cycle it had a row for: {:?}",
        report.refusals
    );
    assert_eq!(report.paths[0].path(), ExecutionPath::HedgedBridging);
    assert!(
        refusals_under(&report, GATE_PATH_ROUTER).is_empty(),
        "a thin hedge was charted as a routing fault: {:?}",
        report.refusals
    );

    let refused = refusals_under(&report, GATE_PATH_EXTENSION);
    assert_eq!(
        refused.len(),
        1,
        "the extension did not refuse: {refused:?}"
    );
    // §33.1's own words about a partial hedge, so an operator reads the
    // blueprint's reason and not a size comparison.
    assert!(
        refused[0].contains("a partial hedge leaves"),
        "the refusal should name what a partial hedge costs: {}",
        refused[0]
    );
    // The book an operator has to go and look at. The gate is handed two
    // sizes and no identity, so without this the cell's answer would be
    // "some local book was short" and the operator would have to re-derive
    // which of the cell's home venues was measured.
    assert!(
        refused[0].contains(&format!("against the hedge book at {VENUE_HEDGE}")),
        "the refusal should name the book that was too thin: {}",
        refused[0]
    );
    // And the leg it was measured for, which is the other identity the gate
    // does not hold.
    assert!(
        refused[0].contains("the mirrored leg in BTC"),
        "the refusal should name the mirrored leg: {}",
        refused[0]
    );
    // The two sizes are the router's and the gate's, and they are the same
    // two: the depth the sweep found and the size the scan planned. A gate
    // reading its own numbers would print a different pair.
    assert!(
        refused[0].contains(&format!(
            "holds {THIN} against the {FIRST_LEG} the first leg needs"
        )),
        "the gate should be measuring the swept depth against the planned first leg: {}",
        refused[0]
    );
    // The consequence, which is the point: nothing of that cycle was sent.
    assert!(
        gateway.placed.is_empty(),
        "legs of a cycle the extension refused reached the venue: {:?}",
        gateway.placed
    );
    assert_eq!(refusals_counted(&metrics, GATE_PATH_EXTENSION), 1);
    assert_eq!(refusals_counted(&metrics, GATE_PATH_ROUTER), 0);
    Ok(())
}

#[test]
fn a_cell_with_no_local_hedge_book_at_all_is_refused_whole_by_the_router() -> Result<()> {
    // Fail closed at the first gate. A hedge that does not exist is a
    // different finding from one that is too thin: the first says the cell
    // cannot execute this cycle any way the table describes, the second says
    // the market is not deep enough today, and they send an operator to two
    // different places. `LiquiditySource::sweep_cost` keeps them apart —
    // `None` is ignorance, `Some(too little)` is a fact about the market —
    // and this asserts the cell keeps them apart too.
    //
    // The premise is asserted by construction: the *only* difference between
    // this cell and the one assigned path 4 above is the hedge books.
    let (mut cell, metrics) = cell_for_row_four(Vec::new())?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_row_four_premises(&cell);

    assert!(
        report.paths.is_empty(),
        "a cycle with no hedge anywhere was assigned a path: {:?}",
        report.paths
    );
    let refused = refusals_under(&report, GATE_PATH_ROUTER);
    assert_eq!(refused.len(), 1, "the router did not refuse: {refused:?}");
    // The table's own message, because the missing fact is upstream of the
    // table rather than a condition inside a row.
    assert!(
        refused[0].contains("no execution path is eligible"),
        "the refusal should be §30.2's own: {}",
        refused[0]
    );
    assert!(
        refusals_under(&report, GATE_PATH_EXTENSION).is_empty(),
        "a cycle the router refused was also charted against the extension: {:?}",
        report.refusals
    );
    assert!(gateway.placed.is_empty());
    assert_eq!(refusals_counted(&metrics, GATE_PATH_ROUTER), 1);

    // The admitting half, and what makes the refusal evidence of a gate
    // rather than of a gate that refuses everything: the same cell with the
    // hedge books present is assigned path 4.
    let (mut stocked, _) = cell_for_row_four(hedge_books(DEEP, THINNER)?)?;
    let stocked_report = stocked.work(t(10), &mut RecordingGateway::default())?;
    assert_eq!(
        stocked_report.paths.len(),
        1,
        "the hedge books are the only difference and the cycle was still refused: {:?}",
        stocked_report.refusals
    );
    assert_eq!(
        stocked_report.paths[0].path(),
        ExecutionPath::HedgedBridging
    );
    Ok(())
}

#[test]
fn the_deepest_local_hedge_is_taken_and_the_same_books_answer_the_same_at_any_pass_time()
-> Result<()> {
    // Two properties of one value, asserted together because the second is
    // only meaningful once the first has something to be stable about.
    //
    // Both home books are too thin, and the deeper of the two is the one the
    // refusal names — a cell that took the first local book it found, or the
    // shallowest, would refuse for a book that was not the best cover it
    // had.
    //
    // And `Cell::local_hedges_for` reads no clock: it is a function of the
    // books, the cell's own venue list and the scanned size. So the same
    // books answer identically at a pass time thirty seconds later, and the
    // verdict is word-for-word the same sentence.
    //
    // The cycle id is the one part that legitimately moves — the scanner
    // stamps it with the instant the cycle was found — so it is split off
    // rather than compared. Splitting it off is asserted rather than
    // tolerated: if the refusal ever stops naming the path this way the
    // `expect` fails, instead of the comparison quietly widening to the
    // whole sentence.
    let mut verdicts = Vec::new();
    let mut cycle_ids = Vec::new();
    for at in [t(10), t(40)] {
        let (mut cell, _) = cell_for_row_four(hedge_books(THIN, THINNER)?)?;
        let mut gateway = RecordingGateway::default();
        let report = cell.work(at, &mut gateway)?;
        assert_row_four_premises(&cell);
        assert_eq!(
            report.paths.len(),
            1,
            "the router refused a cycle it had a row for at {at:?}: {:?}",
            report.refusals
        );
        assert_eq!(report.paths[0].path(), ExecutionPath::HedgedBridging);
        let refused = refusals_under(&report, GATE_PATH_EXTENSION);
        assert_eq!(
            refused.len(),
            1,
            "the extension did not refuse at {at:?}: {refused:?}"
        );
        assert!(gateway.placed.is_empty());
        let (cycle_id, verdict) = refused[0]
            .split_once(" is assigned path ")
            .expect("the refusal names the cycle and then the path it was assigned");
        cycle_ids.push(cycle_id.to_string());
        verdicts.push(verdict.to_string());
    }
    // Premise for the comparison below: the two passes really were different
    // passes, so an equal verdict is a property of the books rather than of
    // the same run being read twice.
    assert_ne!(
        cycle_ids[0], cycle_ids[1],
        "both passes produced the same cycle id, so this compares one pass with itself"
    );
    let messages = verdicts;

    // The deeper of the two home books, named. The premise that there were
    // two to choose between is `hedge_books` above: both are tracked, both
    // are at home, and neither is the venue the local leg trades.
    assert!(
        messages[0].contains(&format!("against the hedge book at {VENUE_HEDGE}")),
        "the deepest local book should be the one measured: {}",
        messages[0]
    );
    assert!(
        !messages[0].contains(&format!("against the hedge book at {VENUE_HEDGE_TWO}")),
        "the shallower local book was measured instead of the deeper one: {}",
        messages[0]
    );
    // And the depth in the message is the deeper book's, not the shallower
    // one's — the venue name alone would pass if the two were swapped in only
    // one of the two places the value is read.
    assert!(
        messages[0].contains(&format!("holds {THIN} against")),
        "the depth reported should be the deeper book's: {}",
        messages[0]
    );
    assert_eq!(
        messages[0], messages[1],
        "the same books answered differently at a different pass time, so something in the \
         hedge read is not a function of the books"
    );
    Ok(())
}
