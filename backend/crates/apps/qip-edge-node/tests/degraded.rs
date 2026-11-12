//! A cell that has lost everything but its venue (REFLEX-052, REFLEX-066).
//!
//! The pieces are each proven somewhere: a tick against a dead centre leaves
//! the cell running (`mesh.rs`), a dark peer suspends only its mirrors
//! (`cross_region.rs`), a partitioned cell spends within its last share
//! (`qip-edge`'s `region_table.rs`). What nothing drove is all of it at once
//! — the centre gone, every peer dark, the payload ageing with no successor
//! — on a cell that still holds a valid envelope, which is the one situation
//! the requirement is about. And what nothing recorded is the degradation
//! itself: the cell's narrowing was sealed only when a payload arrived, and a
//! cell cut off from its centre receives none, so the chain of a cell that
//! had halved its size showed a fresh payload followed by smaller orders and
//! nothing between to say why.
//!
//! There is no model endpoint to cut. The node links no model client at all
//! — `architecture.rs`'s `no_edge_cell_can_reach_a_language_model` — so that
//! dependency is absent rather than severed, and saying so is more honest
//! than stubbing one in order to kill it.
//!
//! Every timeline here ends inside thirty seconds of the venue session's
//! start: nothing in the pass loop answers a heartbeat, and the simulated
//! venue degrades its session after thirty. The slots' own instants are set
//! so their staleness thresholds fall inside that window; the thresholds
//! themselves are the contract's.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, GrantManifest, PolicyItem, PolicyPayload, Slot,
};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Decimal, Duration, ManualClock, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::region::RegionOutlook;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::mesh::{MeshLink, MeshSettings};
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "cut-off-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const KEY: &[u8] = b"degraded-test-key";
/// The most one order may commit under the envelope the cell holds.
const ORDER_LIMIT: &str = "2000";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-CUTOFF")
}

/// The envelope the cell holds: signed, and live until `expires`.
fn grant(expires: Timestamp) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1000000"),
            Decimal::parse(ORDER_LIMIT).expect("a decimal literal"),
            dec!("50000"),
            vec![venue()],
            t(0),
            expires,
            "alice@example.com",
            signature,
        )
    };
    // Bounded as the centre's issuer bounds every grant (R12-CAPITAL-4088):
    // the cell `assemble` builds refuses an envelope without exposure bounds.
    let unsigned = build("")?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        qip_core::Decimal::ONE,
        capital,
    )?)?;
    let loss_limit = unsigned.loss_limit();
    let unsigned = unsigned.with_drawdown(qip_contracts::DrawdownBounds::bounded(
        loss_limit, None, None,
    )?)?;
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), KEY, CELL, t(1))
}

/// The last payload the centre shipped before it went quiet, with each
/// sizing input produced so that its own time to live runs out at the
/// instant named. No payload follows it: the cell's clock is the only thing
/// that moves.
fn last_payload(
    grant_name: &str,
    beliefs_fresh_until: Timestamp,
    causal_fresh_until: Timestamp,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![grant_name.to_string()],
        },
        t(5),
    );
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        beliefs_fresh_until.saturating_sub(PolicyItem::BeliefPriors.time_to_live()),
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        causal_fresh_until.saturating_sub(PolicyItem::CausalDigest.time_to_live()),
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, t(5))
}

/// A link to a centre nothing listens at: bind to learn a free port, then
/// drop the listener so every connect is refused.
fn severed_link() -> Result<MeshLink> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|error| qip_core::error::Error::io(error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| qip_core::error::Error::io(error.to_string()))?;
    drop(listener);
    MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: format!("http://{address}"),
            seed: 3,
        },
        KEY,
        Arc::new(ManualClock::new(t(0))),
        Arc::new(qip_transport::RecordingSleeper::new()),
    )
}

/// A node holding one always-firing strategy under `envelope` and
/// `payload`, at a venue with a two-sided book, with everything central cut
/// before its first pass: the centre refuses every connection, and the
/// region wire cannot be read, which the cell treats as every peer dark.
fn cut_off(
    envelope: VerifiedEnvelope,
    payload: VerifiedPolicy,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed, MeshLink)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = permitting_to_seed(SimulatedGateway::new(venue(), 7, t(0))?, &[&object()])?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;

    // One strategy that always wants ten, so what each pass sends is decided
    // by the cell's own controls and nothing else.
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    node.cell.deploy_with_pricing(
        compiled,
        compiler.into_program(),
        envelope,
        PricingPolicy::Marketable,
    )?;
    node.cell.apply_policy(payload, t(5))?;

    let link = severed_link()?;
    node.cell.apply_region_outlook(
        RegionOutlook::Unreadable("the region wire's mount is gone".to_string()),
        t(9),
    );
    assert_eq!(
        sealed(&node, "region_outlook_changed").len(),
        1,
        "the premise is a cell that has been told its peers are dark"
    );

    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    Ok((node, gateway, feed, link))
}

fn sealed(node: &NodeAssembly, kind: &str) -> Vec<Decision> {
    node.cell
        .journal()
        .entries()
        .iter()
        .filter(|entry| entry.decision.kind() == kind)
        .map(|entry| entry.decision.clone())
        .collect()
}

/// One pass, then one exchange with the centre that is not there.
fn pass_cut_off(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    link: &mut MeshLink,
    stats: &mut PassStats,
    now: Timestamp,
) -> Result<WorkReport> {
    let outcome = run_pass(&mut node.cell, gateway, feed, None, stats, now)?;
    let PassOutcome::Ran { report, breaks, .. } = outcome else {
        panic!("a cell cut off from everything stopped running: {outcome:?}");
    };
    assert!(breaks.is_empty(), "{breaks:?}");
    let tick = link.exchange(&mut node.cell, &report, now);
    // Either the connect was refused, or the link has stopped trying: after
    // enough refusals its circuit opens and the exchange sends nothing.
    assert!(
        tick.poll_error.is_some() || tick.delta.as_deref() == Some("circuit_open"),
        "the premise is a centre that cannot be reached: {tick:?}"
    );
    assert!(
        tick.renewed.is_empty() && tick.policies.is_empty() && tick.halts == 0,
        "the premise is a cell that receives nothing from its centre: {tick:?}"
    );
    Ok(*report)
}

#[test]
fn a_cell_cut_off_from_everything_keeps_placing_paper_orders_inside_its_envelope_and_journals_that_it_is_degraded()
-> Result<()> {
    // The envelope is live for the hour and the causal graph for the whole
    // test; the belief priors age out at `t(15)`.
    let envelope = grant(t(3600))?;
    let grant_name = envelope.signature().to_string();
    let (mut node, mut gateway, mut feed, mut link) =
        cut_off(envelope, last_payload(&grant_name, t(15), t(3600))?)?;
    let mut stats = PassStats::default();
    let order_limit = Decimal::parse(ORDER_LIMIT).expect("a decimal literal");

    // The first pass: the last payload is still fresh, so the cell sizes at
    // what the strategy asked for and the chain has nothing to add.
    let fresh = pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(10),
    )?;
    assert_eq!(
        node.cell.narrowing(t(10)).sizing_multiplier(),
        dec!("1"),
        "the premise is a payload whose sizing inputs are fresh at the first pass"
    );
    assert_eq!(
        fresh.orders.len(),
        1,
        "the cut-off cell placed nothing: {:?}",
        fresh.refusals
    );
    assert_eq!(fresh.orders[0].quantity, dec!("10"));
    assert!(
        sealed(&node, "degradation_changed").is_empty(),
        "the premise is a chain that has not yet had a degradation to record"
    );

    // Ten seconds on, nothing has arrived and the belief priors have aged
    // out. The cell keeps deciding — at half size — and says so.
    let aged = pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(20),
    )?;
    assert!(
        !node.cell.is_halted(),
        "losing the centre and its peers halted the cell"
    );
    assert_eq!(
        aged.orders.len(),
        1,
        "the degraded cell stopped placing orders while its envelope was live: {:?}",
        aged.refusals
    );
    let order = &aged.orders[0];
    assert!(order.simulated, "the order was not a paper order");
    assert_eq!(
        order.quantity,
        dec!("5"),
        "the degraded cell did not narrow what it sent"
    );
    for placed in fresh.orders.iter().chain(&aged.orders) {
        assert!(
            placed.quantity * placed.price <= order_limit,
            "an order of {} at {} is outside the envelope's {order_limit} per order",
            placed.quantity,
            placed.price
        );
    }
    assert_eq!(gateway.submitted_count(), 2);

    let degraded = sealed(&node, "degradation_changed");
    assert_eq!(
        degraded.len(),
        1,
        "the cell narrowed on its own clock and the chain does not say so: {degraded:?}"
    );
    let Decision::DegradationChanged {
        sequence,
        narrowed,
        sizing_multiplier,
    } = &degraded[0]
    else {
        panic!("`sealed` returned another kind: {:?}", degraded[0]);
    };
    assert_eq!(
        *sequence,
        Some(1),
        "the entry does not name the payload that aged"
    );
    assert!(
        narrowed.iter().any(|name| name == "belief_state:stale"),
        "the entry does not name what went stale: {narrowed:?}"
    );
    assert!(
        !narrowed
            .iter()
            .any(|name| name.starts_with("causal_graph:")),
        "the entry names a capability that is still fresh: {narrowed:?}"
    );
    assert_eq!(sizing_multiplier, "0.5");

    // Once, at the transition: a third pass under the same reading adds no
    // second entry.
    pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(22),
    )?;
    assert_eq!(
        sealed(&node, "degradation_changed").len(),
        1,
        "an unchanged reading was journaled again"
    );
    assert_eq!(stats.passes, 3);
    Ok(())
}

/// REFLEX-066's own check, followed through one outage: the sizing
/// multiplier falls a step as each input crosses its own staleness
/// threshold, every order is no larger than the step in force when it was
/// sent, and at the envelope's expiry the cell sends nothing at all.
///
/// What this does not show, because the cell does not do it: the envelope's
/// age narrows nothing until the instant it stops everything, and a stale
/// input and a missing one narrow alike. Those are the table's, and they are
/// stated in the register rather than papered over here.
#[test]
fn a_cut_off_cells_orders_shrink_a_step_as_each_input_ages_and_stop_when_its_envelope_expires()
-> Result<()> {
    // Beliefs age out at `t(15)`, the causal graph at `t(19)`, and the
    // envelope expires at `t(25)`.
    let envelope = grant(t(25))?;
    let grant_name = envelope.signature().to_string();
    let (mut node, mut gateway, mut feed, mut link) =
        cut_off(envelope, last_payload(&grant_name, t(15), t(19))?)?;
    let mut stats = PassStats::default();

    let mut sent: Vec<(i64, Decimal, Decimal)> = Vec::new();
    for second in [10, 16, 20, 24] {
        let multiplier = node.cell.narrowing(t(second)).sizing_multiplier();
        let report = pass_cut_off(
            &mut node,
            &mut gateway,
            &mut feed,
            &mut link,
            &mut stats,
            t(second),
        )?;
        assert_eq!(
            report.orders.len(),
            1,
            "the cell stopped placing orders at t({second}) with its envelope still live: {:?}",
            report.refusals
        );
        sent.push((second, multiplier, report.orders[0].quantity));
    }

    // The steps, as the cell read them: whole, then the belief row, then the
    // causal row compounded with it, then nothing further to lose.
    let multipliers: Vec<Decimal> = sent.iter().map(|(_, multiplier, _)| *multiplier).collect();
    assert_eq!(
        multipliers,
        vec![dec!("1"), dec!("0.5"), dec!("0.375"), dec!("0.375")],
        "the multiplier did not fall a step as each input aged: {sent:?}"
    );
    // And what was sent: the strategy's ten at the step in force, never more.
    for (second, multiplier, quantity) in &sent {
        assert_eq!(
            *quantity,
            dec!("10") * *multiplier,
            "the order at t({second}) is not the size the narrowed limit allows"
        );
    }
    // One sealed entry per step down, each naming the multiplier it left the
    // cell at.
    let steps: Vec<String> = sealed(&node, "degradation_changed")
        .into_iter()
        .filter_map(|decision| match decision {
            Decision::DegradationChanged {
                sizing_multiplier, ..
            } => Some(sizing_multiplier),
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        vec!["0.5".to_string(), "0.375".to_string()],
        "the chain does not hold one entry per step the cell narrowed by"
    );

    // Past the envelope's expiry the cell takes no new risk.
    let submitted = gateway.submitted_count();
    let expired = pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(26),
    )?;
    assert!(
        expired.orders.is_empty(),
        "an order was placed under an envelope that had expired: {:?}",
        expired.orders
    );
    assert!(
        expired
            .refusals
            .iter()
            .any(|(gate, _)| gate == "envelope_expiry"),
        "the cell went quiet at its envelope's expiry without saying why: {:?}",
        expired.refusals
    );
    assert_eq!(gateway.submitted_count(), submitted);
    Ok(())
}

/// `gateway`, permitted to list each of `objects` on demand and to seed
/// liquidity into it: a recorded venue, product and jurisdiction permission
/// for each action on each instrument (R12-COVERAGE-5107). `Seed` as well as
/// `List`, because the tests here seed the touch they trade against and
/// permission to list is not permission to seed. A fixture; the refusals
/// without either are proven in qip-edge-node's tests/gateway.rs.
fn permitting_to_seed(
    gateway: SimulatedGateway,
    objects: &[&ObjectId],
) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for object in objects {
        for action in [
            CreationAction::Create,
            CreationAction::List,
            CreationAction::Seed,
        ] {
            let request = CreationRequest::new(action, venue.as_str(), object.as_str(), "paper");
            for authority in CreationAuthority::ALL {
                permissions.record(authority, &request, "test fixture")?;
            }
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}

// ---- the hedge narrowing (ADR 0126 decision 4, R12-CAPITAL-10098) -------

use qip_contracts::policy::{HedgeNarrowing, HedgeTarget};
use qip_edge::hedge::{GATE_HEDGE_FRAME, GATE_HEDGE_TARGET, VerifiedHedge};

fn narrowing(
    cell: &str,
    plan: &str,
    issued: Timestamp,
    deadline: Timestamp,
    at: VenueId,
    target_net: Decimal,
) -> Result<HedgeNarrowing> {
    HedgeNarrowing::new(
        cell,
        plan,
        issued,
        deadline,
        vec![HedgeTarget {
            venue: at,
            object_id: object(),
            target_net,
        }],
    )
    .signed(KEY)
}

/// A cut-off cell that has bought once, and what it then holds.
fn cut_off_and_long() -> Result<(
    NodeAssembly,
    SimulatedGateway,
    SimulatedFeed,
    MeshLink,
    PassStats,
    Decimal,
)> {
    let envelope = grant(t(3600))?;
    let grant_name = envelope.signature().to_string();
    let (mut node, mut gateway, mut feed, mut link) =
        cut_off(envelope, last_payload(&grant_name, t(3600), t(3600))?)?;
    let mut stats = PassStats::default();
    let first = pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(10),
    )?;
    assert_eq!(first.orders.len(), 1, "{:?}", first.refusals);
    let held = node.cell.position(&venue(), &object());
    assert!(
        held.is_positive(),
        "the premise is a cell holding a long position a hedge can reduce; it holds {held}"
    );
    Ok((node, gateway, feed, link, stats, held))
}

/// The row's bar. Without the gate a hedge plan made at the centre changes
/// nothing a cell does: the strategy keeps adding to the position the plan
/// exists to reduce, inside an envelope that has no reason to refuse it.
#[test]
fn a_cut_off_cell_applies_a_hedge_target_and_refuses_an_order_moving_away_from_it_until_the_deadline()
-> Result<()> {
    let (mut node, mut gateway, mut feed, mut link, mut stats, held) = cut_off_and_long()?;
    let sent_before = gateway.submitted_count();

    // Flat by t(25): every further buy moves the net away from the target.
    let hedge = VerifiedHedge::verify(
        narrowing(CELL, "HP-1", t(11), t(25), venue(), Decimal::ZERO)?,
        KEY,
        CELL,
        t(12),
    )?;
    node.cell.apply_hedge(hedge, t(12))?;
    let applied = sealed(&node, "hedge_applied");
    assert_eq!(applied.len(), 1, "{applied:?}");
    let Decision::HedgeApplied {
        plan_id,
        deadline,
        targets,
    } = &applied[0]
    else {
        panic!("not a hedge application: {:?}", applied[0]);
    };
    assert_eq!(plan_id, "HP-1");
    assert_eq!(*deadline, t(25));
    assert_eq!(targets, &vec![format!("{VENUE}/{}=0", object().as_str())]);

    let narrowed = pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(20),
    )?;
    assert!(
        narrowed.orders.is_empty(),
        "the cell bought away from its hedge target: {:?}",
        narrowed.orders
    );
    assert_eq!(
        gateway.submitted_count(),
        sent_before,
        "an order reached the gateway past the hedge target"
    );
    let refusals: Vec<&(String, String)> = narrowed
        .refusals
        .iter()
        .filter(|(gate, _)| gate == GATE_HEDGE_TARGET)
        .collect();
    assert_eq!(refusals.len(), 1, "{:?}", narrowed.refusals);
    assert!(
        refusals[0].1.contains("HP-1") && refusals[0].1.contains(&format!("net of {held} away")),
        "the refusal does not name the plan and the holding: {}",
        refusals[0].1
    );
    assert_eq!(node.cell.position(&venue(), &object()), held);

    // Past the deadline the narrowing binds nothing and the cell is as
    // bounded as its envelope leaves it.
    let lapsed = pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(26),
    )?;
    assert_eq!(
        lapsed.orders.len(),
        1,
        "the narrowing outlived its deadline: {:?}",
        lapsed.refusals
    );
    Ok(())
}

/// The acceptance rule. Each of these frames is correctly signed by the key
/// the cell trusts, which is the point: a signature says who sent a
/// narrowing, not that this cell can honour it.
#[test]
fn a_cell_refuses_a_hedge_frame_for_another_cell_past_its_deadline_for_a_venue_it_is_not_granted_or_adding_exposure()
-> Result<()> {
    let (mut node, mut gateway, mut feed, mut link, mut stats, held) = cut_off_and_long()?;

    let elsewhere = VerifiedHedge::verify(
        narrowing("another-cell", "HP-X", t(11), t(40), venue(), Decimal::ZERO)?,
        KEY,
        CELL,
        t(12),
    );
    assert!(
        elsewhere
            .as_ref()
            .is_err_and(|error| error.message().contains("another-cell")),
        "{elsewhere:?}"
    );
    let forged = VerifiedHedge::verify(
        narrowing(CELL, "HP-F", t(11), t(40), venue(), Decimal::ZERO)?,
        b"a-key-this-cell-never-held",
        CELL,
        t(12),
    );
    assert!(forged.is_err(), "{forged:?}");

    let refused = [
        (
            "past its deadline",
            narrowing(CELL, "HP-LATE", t(1), t(12), venue(), Decimal::ZERO)?,
        ),
        (
            "no live envelope",
            narrowing(
                CELL,
                "HP-VENUE",
                t(11),
                t(40),
                VenueId::new("XNYS"),
                Decimal::ZERO,
            )?,
        ),
        (
            "between the holding and zero",
            narrowing(CELL, "HP-ADD", t(11), t(40), venue(), held + dec!("1"))?,
        ),
        (
            "between the holding and zero",
            narrowing(CELL, "HP-FLIP", t(11), t(40), venue(), dec!("-1"))?,
        ),
    ];
    for (index, (why, frame)) in refused.into_iter().enumerate() {
        let plan = frame.plan_id.clone();
        let verified = VerifiedHedge::verify(frame, KEY, CELL, t(12))?;
        let outcome = node.cell.apply_hedge(verified, t(12));
        assert!(
            outcome
                .as_ref()
                .is_err_and(|error| error.message().contains(why)),
            "{plan}: {outcome:?}"
        );
        let journaled: Vec<Decision> = sealed(&node, "refused")
            .into_iter()
            .filter(|decision| {
                matches!(decision, Decision::Refused { gate, reason }
                    if gate == GATE_HEDGE_FRAME && reason.contains(&plan))
            })
            .collect();
        assert_eq!(
            journaled.len(),
            1,
            "{plan} (frame {index}) is not journaled"
        );
    }
    assert!(sealed(&node, "hedge_applied").is_empty());
    assert!(node.cell.live_hedge(t(12)).is_none());

    // Nothing was applied, so the cell still decides alone: it buys.
    let free = pass_cut_off(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut link,
        &mut stats,
        t(20),
    )?;
    assert_eq!(free.orders.len(), 1, "{:?}", free.refusals);

    // A newer plan applies; the older one replayed after it does not put
    // its target back.
    let held = node.cell.position(&venue(), &object());
    let newer = VerifiedHedge::verify(
        narrowing(CELL, "HP-2", t(22), t(60), venue(), held)?,
        KEY,
        CELL,
        t(23),
    )?;
    node.cell.apply_hedge(newer, t(23))?;
    let replay = VerifiedHedge::verify(
        narrowing(CELL, "HP-1", t(21), t(60), venue(), Decimal::ZERO)?,
        KEY,
        CELL,
        t(24),
    )?;
    let outcome = node.cell.apply_hedge(replay, t(24));
    assert!(
        outcome
            .as_ref()
            .is_err_and(|error| error.message().contains("at or behind")),
        "{outcome:?}"
    );
    assert_eq!(
        node.cell.live_hedge(t(24)).map(VerifiedHedge::plan_id),
        Some("HP-2")
    );
    Ok(())
}

/// The node's half of R12-CAPITAL-10098 (ADR 0407): a narrowing the policy
/// downlink verified is applied through `apply_inbound`, the seam the node's
/// decision loop runs, and one the cell refuses is reported as refused on
/// the tick rather than dropped. Until this test the arm that hands a
/// verified narrowing to `Cell::apply_hedge` was untested; every hedge test
/// called `apply_hedge` itself.
#[test]
fn a_verified_hedge_narrowing_is_applied_through_the_nodes_inbound_seam_and_a_refused_one_is_reported()
-> Result<()> {
    use qip_edge::mesh::{DownlinkBatch, PolicyBatch};
    use qip_edge_node::mesh::{MeshInbound, apply_inbound};

    let (mut node, _gateway, _feed, _link, _stats, _held) = cut_off_and_long()?;
    assert!(
        node.cell.live_hedge(t(12)).is_none(),
        "premise: the cell holds no narrowing before the poll"
    );
    let applied = VerifiedHedge::verify(
        narrowing(CELL, "HP-IN", t(11), t(25), venue(), Decimal::ZERO)?,
        KEY,
        CELL,
        t(12),
    )?;
    let ungranted = VerifiedHedge::verify(
        narrowing(
            CELL,
            "HP-OUT",
            t(11).saturating_add(Duration::from_secs(1)),
            t(25),
            VenueId::new("NOT-GRANTED"),
            Decimal::ZERO,
        )?,
        KEY,
        CELL,
        t(12),
    )?;
    let policy = PolicyBatch {
        hedges: vec![applied, ungranted],
        ..PolicyBatch::default()
    };
    let tick = apply_inbound(
        &mut node.cell,
        MeshInbound::from_batches(DownlinkBatch::default(), policy),
        t(12),
        None,
        None,
    );
    assert_eq!(tick.hedges, vec!["HP-IN".to_string()], "{:?}", tick.refused);
    assert!(
        tick.refused
            .iter()
            .any(|line| line.starts_with("hedge HP-OUT") && line.contains("NOT-GRANTED")),
        "the refused narrowing was not reported: {:?}",
        tick.refused
    );
    let live = node
        .cell
        .live_hedge(t(12))
        .ok_or_else(|| qip_core::error::Error::not_found("the applied narrowing"))?;
    assert_eq!(live.plan_id(), "HP-IN");
    Ok(())
}
