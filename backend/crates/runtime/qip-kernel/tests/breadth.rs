//! The breadth plane, driven through the platform the binaries assemble:
//! the Market Factory (R12-EXEC-5080/-5081, R12-COVERAGE-5107), event
//! markets (R12-EVENT-5082 to -5089) and commerce research
//! (R12-COMMERCE-5090 to -5097).
//!
//! Every test installs a plan through `Platform::install_breadth`, the call
//! `qip-deepbrain` makes at start-up, and then runs real cycles: a test that
//! called the planes directly would pass with the cycle hooks deleted.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

use qip_contracts::VenueId;
use qip_core::error::Result;
use qip_core::{Context, Decimal, Duration, Timestamp, dec};
use qip_events::Topic;
use qip_execution_engine::creation::CreationPermissions;
use qip_execution_engine::market_creation::{FactoryAction, MarketStage};
use qip_financial::physical::{
    Customs, Leg, LogisticsTerms, MarketplaceFees, Returns, Spoilage, TransportMode,
};
use qip_financial::universe::Universe;
use qip_kernel::breadth::{BreadthPlan, EventMarketPlan};
use qip_kernel::commerce_research::{
    AccountEntry, CommercePlan, CommerceSimulated, LogisticsEntry, ResaleEntry,
    SYNTHETIC_COUNTERFEIT_SELLER, SyntheticCommerceFeed,
};
use qip_kernel::event_markets::{EventMarketPlane, EventMarketsSimulated};
use qip_kernel::market_factory_stage::{
    FactoryPlan, MarketEntry, MarketFactoryRecorded, PermissionEntry,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_prediction::access::{AccessBook, EntityRecord, Jurisdiction, VenueKind, VenuePolicy};
use qip_prediction::{
    OracleIdentity, OracleKind, ResolutionState, VenueApiAdapter, VenueApiConfig,
};
use qip_risk::limits::LimitSet;
use qip_streaming::envelope::StreamEnvelope;

const VENUE: &str = "simulated-venue";
const PRODUCT: &str = "obj-CREATED";
const EVENT_VENUE: &str = "SYNTH-PREDICT";
const EVENT_PRODUCTS: [&str; 3] = [
    "SYNTH-PREDICT-CUT",
    "SYNTH-PREDICT-HOLD",
    "SYNTH-PREDICT-HIKE",
];

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

fn factory_plan(mark_source: &str) -> FactoryPlan {
    let mut permissions = Vec::new();
    for action in ["create", "list", "seed"] {
        for authority in ["venue", "product", "jurisdiction"] {
            permissions.push(PermissionEntry {
                authority: authority.to_string(),
                action: action.to_string(),
                venue: VENUE.to_string(),
                product: PRODUCT.to_string(),
                jurisdiction: "GB".to_string(),
                reference: format!("{authority} approval for {action}"),
            });
        }
    }
    FactoryPlan {
        permissions,
        markets: vec![MarketEntry {
            venue: VENUE.to_string(),
            product: PRODUCT.to_string(),
            jurisdiction: "GB".to_string(),
            reference_price: dec!("100"),
            half_spread: dec!("0.5"),
            level_step: dec!("0.25"),
            level_size: dec!("10"),
            levels: 3,
            tolerance: dec!("0.05"),
            inventory: dec!("30"),
            mark_source: mark_source.to_string(),
            event_contract: false,
        }],
    }
}

fn factory_records(platform: &Platform) -> Result<Vec<MarketFactoryRecorded>> {
    let mut records = Vec::new();
    for event in platform.event_log().by_topic(Topic::ComplianceEvaluated) {
        if let Ok(decoded) = StreamEnvelope::from_frame(event)?.decode::<MarketFactoryRecorded>() {
            records.push(decoded.body);
        }
    }
    Ok(records)
}

/// R12-EXEC-5080 through the platform: the plan's market is created, listed
/// and seeded on the desk's own simulated venue, its pricing rule is in the
/// hash-chained log, and the desk's venue shows the seeded book.
///
/// Mutation: make `MarketFactory::seed` skip `venue.list_book(book)` and the
/// venue rests no book (`Platform::listed_book` is `None`); drop the drain in `install_breadth` and the log
/// holds no factory record.
#[test]
fn a_planned_market_is_created_listed_and_seeded_on_the_simulated_venue_and_journaled() -> Result<()>
{
    let mut platform = platform()?;
    // Premise: nothing is installed until a plan is.
    assert!(platform.market_factory().is_none());
    let lines = platform.install_breadth(
        &BreadthPlan {
            market_factory: Some(factory_plan("obj-NEVER-OBSERVED")),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    assert_eq!(lines.len(), 1, "{lines:?}");
    let stage = platform
        .market_factory()
        .expect("the plan installed a factory");
    let market = stage
        .factory()
        .market(PRODUCT)
        .expect("the planned market exists");
    assert_eq!(market.stage, MarketStage::Seeded);
    assert_eq!(market.inventory, dec!("30"));
    // The venue itself rests the rule's ladder: three levels a side, the
    // touch half a point either side of 100. Until the round-2 mutation
    // audit nothing here read the venue, and a factory that journaled
    // `Seeded` without listing the book passed.
    let book = platform
        .listed_book(PRODUCT)
        .expect("the desk's venue rests the seeded book");
    assert_eq!((book.bids.len(), book.asks.len()), (3, 3));
    assert_eq!(book.bids[0].price, dec!("99.5"));
    assert_eq!(book.asks[0].price, dec!("100.5"));

    // The venue's own book, not the factory's account of it. This test's
    // mutation note claimed it read the venue's book until 2026-10-11 and it
    // never did (the audit's EXEC-5080 finding): every assertion above is on
    // the factory's record, which says "seeded" whether or not the venue
    // holds a single order.
    let book = platform
        .listed_book(PRODUCT)
        .expect("the venue lists the created instrument");
    assert!(
        book.total_depth().is_positive(),
        "the venue lists an empty book for a seeded market: {book:?}"
    );
    assert_eq!(book.mid(), Some(dec!("100")), "{book:?}");
    assert_eq!(book.spread(), Some(dec!("1")), "{book:?}");

    let records = factory_records(&platform)?;
    let actions: Vec<FactoryAction> = records.iter().map(|r| r.record.action).collect();
    assert_eq!(
        actions,
        vec![
            FactoryAction::Created,
            FactoryAction::Listed,
            FactoryAction::Seeded
        ]
    );
    assert_eq!(records[0].record.rule.reference_price(), dec!("100"));
    assert!(platform.event_log().verify_chain().is_ok());
    Ok(())
}

/// R12-EXEC-5081: a permission revoked after creation withdraws the market
/// at the next cycle's SIMULATE stage, and the withdrawal is journaled.
///
/// Mutation: delete `self.breadth_simulate(now, &mut simulated)` from
/// `run_cycle` (or skip the permission loop in `MarketFactory::assess`) and
/// the market is still seeded after the cycle.
#[test]
fn a_created_market_whose_permission_is_revoked_is_withdrawn_by_the_next_cycle() -> Result<()> {
    let mut platform = platform()?;
    platform.install_breadth(
        &BreadthPlan {
            market_factory: Some(factory_plan("obj-NEVER-OBSERVED")),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    platform
        .market_factory_mut()
        .expect("installed")
        .set_permissions(CreationPermissions::new());
    platform.run_cycle(start());
    let market = platform
        .market_factory()
        .and_then(|stage| stage.factory().market(PRODUCT))
        .expect("the market is still recorded");
    assert_eq!(market.stage, MarketStage::Withdrawn);
    let records = factory_records(&platform)?;
    let flagged = records
        .iter()
        .find(|record| record.record.action == FactoryAction::Flagged)
        .expect("the withdrawal was flagged in the log");
    // The legal finding comes first, before the economic one.
    assert!(
        flagged.record.detail.contains("lapsed"),
        "{}",
        flagged.record.detail
    );
    Ok(())
}

/// R12-EXEC-5081, economic half: a seeded market nothing marks is withdrawn
/// rather than left quoting prices nobody checked. Mutation: return `None`
/// for an unmarked market in `assess` and it stays seeded.
#[test]
fn a_seeded_market_with_no_mark_is_withdrawn_at_the_first_review() -> Result<()> {
    let mut platform = platform()?;
    platform.install_breadth(
        &BreadthPlan {
            market_factory: Some(factory_plan("obj-NEVER-OBSERVED")),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    // Premise: the mark source has never been observed.
    assert!(!platform.price_history().contains_key("obj-NEVER-OBSERVED"));
    platform.run_cycle(start());
    let records = factory_records(&platform)?;
    assert!(
        records
            .iter()
            .any(|record| record.record.action == FactoryAction::Withdrawn
                && record.record.detail.contains("no mark")),
        "{records:?}"
    );
    Ok(())
}

/// COVERAGE-5107 at installation: a plan missing one authority's permission
/// is refused whole and creates nothing. Mutation: drop the jurisdiction
/// check from `CreationPermissions::admit` and the plan installs.
#[test]
fn a_plan_missing_a_permission_is_refused_whole_and_creates_nothing() -> Result<()> {
    let mut platform = platform()?;
    let mut plan = factory_plan("obj-NEVER-OBSERVED");
    plan.permissions
        .retain(|entry| !(entry.action == "list" && entry.authority == "jurisdiction"));
    let refused = platform
        .install_breadth(
            &BreadthPlan {
                market_factory: Some(plan),
                ..BreadthPlan::default()
            },
            start(),
        )
        .expect_err("a plan missing the jurisdiction's permission to list installed");
    assert!(
        refused.message().contains("jurisdiction"),
        "{}",
        refused.message()
    );
    assert!(platform.market_factory().is_none());
    assert!(factory_records(&platform)?.is_empty());
    Ok(())
}

fn event_plan(entity: EntityRecord, kind: VenueKind) -> Result<EventMarketPlan> {
    let gb = Jurisdiction::new("GB")?;
    Ok(EventMarketPlan {
        source: "synthetic".to_string(),
        entity: "desk".to_string(),
        policies: vec![VenuePolicy {
            venue: VenueId::new(EVENT_VENUE),
            kind,
            creation: BTreeSet::new(),
            trading: BTreeSet::from([gb]),
            products: EVENT_PRODUCTS.iter().map(|p| (*p).to_string()).collect(),
            minimum_age: Some(18),
        }],
        entities: BTreeMap::from([("desk".to_string(), entity)]),
    })
}

fn eligible() -> Result<EntityRecord> {
    Ok(EntityRecord {
        jurisdiction: Jurisdiction::new("GB")?,
        eligible: true,
        age_years: Some(30),
        venues: BTreeSet::from([VenueId::new(EVENT_VENUE)]),
    })
}

fn simulated_event_records(platform: &Platform) -> Result<Vec<EventMarketsSimulated>> {
    let mut records = Vec::new();
    for event in platform.event_log().by_topic(Topic::ScenarioEvaluated) {
        if let Ok(decoded) = StreamEnvelope::from_frame(event)?.decode::<EventMarketsSimulated>() {
            records.push(decoded.body);
        }
    }
    Ok(records)
}

fn run_event_cycle(
    entity: EntityRecord,
    kind: VenueKind,
) -> Result<(Platform, EventMarketsSimulated)> {
    let mut platform = platform()?;
    platform.install_breadth(
        &BreadthPlan {
            event_markets: Some(event_plan(entity, kind)?),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    platform.run_cycle(start());
    let record = simulated_event_records(&platform)?
        .pop()
        .expect("SIMULATE journaled the event markets");
    Ok((platform, record))
}

/// R12-EVENT-5082, -5084, -5085, -5086 in one cycle: DISCOVER records the
/// listed market's definition at the instant it became knowable, SIMULATE
/// prices every outcome from a distribution summing to one, and paper quotes
/// rest because the access book admits the entity.
///
/// Mutation: delete `self.breadth_discover(now, &mut discovered)` from
/// `run_cycle` and nothing is listed, so nothing is priced; skip
/// `definitions.record` and the store is empty.
#[test]
fn a_cycle_ingests_an_event_market_prices_every_outcome_and_rests_admitted_paper_quotes()
-> Result<()> {
    let (platform, record) = run_event_cycle(eligible()?, VenueKind::Event)?;
    let plane = platform.event_markets().expect("installed");
    let market = plane
        .markets()
        .next()
        .expect("the synthetic market was listed");
    let definition = plane
        .definitions()
        .definition_as_of(market.market_id.as_str(), start(), start())
        .expect("the definition is readable at the instant it was listed");
    assert_eq!(definition.outcome_set.len(), 3);
    // Not readable before it was knowable.
    assert!(
        plane
            .definitions()
            .definition_as_of(
                market.market_id.as_str(),
                start(),
                Timestamp::from_secs(1_760_000_000 - 1)
            )
            .is_none()
    );
    assert!(
        !record.decision_grade,
        "a synthetic source is research only"
    );
    let valuation = record.valuations.first().expect("the market was priced");
    let total: Decimal = valuation.fair_values.values().copied().sum();
    // Unit payoff: the fair values are the probabilities and sum to one.
    assert_eq!(total, Decimal::ONE);
    assert_eq!(record.quotes.len(), 3, "{record:?}");
    assert!(record.refusals.is_empty(), "{:?}", record.refusals);
    Ok(())
}

/// R12-EVENT-5088 and -5089: a venue recorded as regulated wagering is
/// refused on the event path, as is an underage entity, and the price of
/// every outcome is identical whether the entity may trade or not.
///
/// Mutation: route by venue id only in `AccessBook::admit` (drop the kind
/// check) and the wagering venue's quotes rest; read the access book in
/// `EventMarketPlane::price` and the valuations differ.
#[test]
fn wagering_venues_and_ineligible_entities_are_refused_and_the_price_does_not_depend_on_access()
-> Result<()> {
    let (_, admitted) = run_event_cycle(eligible()?, VenueKind::Event)?;
    let (_, wagering) = run_event_cycle(eligible()?, VenueKind::Wagering)?;
    let mut minor = eligible()?;
    minor.age_years = Some(16);
    let (_, underage) = run_event_cycle(minor, VenueKind::Event)?;

    // Premise: the admitted run quoted, so an empty set below is a refusal.
    assert!(!admitted.quotes.is_empty());
    for (refused, why) in [(&wagering, "Wagering venue"), (&underage, "under the 18")] {
        assert!(refused.quotes.is_empty(), "{why}: {refused:?}");
        assert!(refused.fills.is_empty(), "{why}: {refused:?}");
        assert!(
            refused.refusals.iter().any(|reason| reason.contains(why)),
            "{why}: {:?}",
            refused.refusals
        );
        assert_eq!(
            refused.valuations, admitted.valuations,
            "the price moved with availability"
        );
    }
    Ok(())
}

/// R12-EVENT-5083: the oracle's report makes the resolution proposed, not
/// final, and it becomes final only once the dispute window has closed.
/// Mutation: finalise without the window check and the first state read is
/// already final.
#[test]
fn a_reported_event_is_proposed_until_its_dispute_window_closes_and_only_then_final() -> Result<()>
{
    let mut platform = platform()?;
    platform.install_breadth(
        &BreadthPlan {
            event_markets: Some(event_plan(eligible()?, VenueKind::Event)?),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    platform.run_cycle(start());
    let resolves = start().saturating_add(Duration::from_days(30));
    platform.run_cycle(resolves);
    let state = |platform: &Platform| {
        let plane = platform.event_markets().expect("installed");
        let market = plane.markets().next().expect("listed").market_id.clone();
        plane
            .resolution(&market)
            .map(|resolution| resolution.state().as_str().to_string())
    };
    assert_eq!(state(&platform).as_deref(), Some("proposed"));
    platform.run_cycle(resolves.saturating_add(Duration::from_hours(3)));
    assert_eq!(state(&platform).as_deref(), Some("final"));
    let plane = platform.event_markets().expect("installed");
    let market = plane.markets().next().expect("listed");
    assert!(matches!(
        plane.resolution(&market.market_id).map(|r| r.state()),
        Some(ResolutionState::Final { .. })
    ));
    Ok(())
}

/// R12-EVENT plane door: a real source with no licensing decision is refused
/// before it is polled, and a plan naming one is refused at installation.
/// Mutation: treat a missing decision as synthetic in `open` and the plane
/// opens.
#[test]
fn a_real_event_source_without_a_licensing_evaluation_is_refused_before_it_is_polled() -> Result<()>
{
    let adapter =
        VenueApiAdapter::new(VenueApiConfig::standard(VenueId::new("KALSHI")), true, true);
    let refused = EventMarketPlane::open(
        Box::new(adapter),
        None,
        AccessBook::new(),
        "desk",
        OracleIdentity::new("o", OracleKind::Optimistic, Duration::from_hours(2), 0.9)?,
    )
    .expect_err("an unevaluated real source opened");
    assert!(
        refused.message().contains("no licensing evaluation"),
        "{}",
        refused.message()
    );
    let mut platform = platform()?;
    let mut plan = event_plan(eligible()?, VenueKind::Event)?;
    plan.source = "kalshi".to_string();
    assert!(
        platform
            .install_breadth(
                &BreadthPlan {
                    event_markets: Some(plan),
                    ..BreadthPlan::default()
                },
                start(),
            )
            .is_err()
    );
    assert!(platform.event_markets().is_none());
    Ok(())
}

fn commerce_plan() -> Result<CommercePlan> {
    Ok(CommercePlan {
        source: "synthetic".to_string(),
        owner: "desk".to_string(),
        terms: LogisticsTerms {
            route: vec![Leg {
                from: "Rotterdam".into(),
                to: "London".into(),
                mode: TransportMode::Road,
                days: 1,
                freight_per_unit: dec!("1"),
                freight_per_shipment: Decimal::ZERO,
            }],
            customs: Customs {
                duty_rate: Decimal::ZERO,
                clearance_fee: Decimal::ZERO,
                clearance_days: 0,
            },
            spoilage: Spoilage::none("durable goods, stated by the desk")?,
            storage_per_unit_per_day: Decimal::ZERO,
            storage_days: 0,
            fees: MarketplaceFees {
                ad_valorem: dec!("0.05"),
                per_unit: Decimal::ZERO,
                per_consignment: Decimal::ZERO,
            },
            returns: Returns {
                rate: Decimal::ZERO,
                cost_per_unit: Decimal::ZERO,
                recovery_rate: Decimal::ONE,
            },
        },
        quantity: dec!("2"),
        days_on_market: 2,
        daily_capital_cost: dec!("0.0001"),
        purchase_tax_rate: Decimal::ZERO,
        account: AccountEntry {
            id: "paper-account".to_string(),
            identity_class: "desk".to_string(),
            merchants: SyntheticCommerceFeed::venues().into_iter().collect(),
            per_purchase_limit: dec!("10000"),
        },
        total_ceiling: dec!("100000"),
        fraud_merchants: BTreeSet::new(),
        counterfeit_gtins: BTreeSet::new(),
        counterfeit_sellers: BTreeSet::from([SYNTHETIC_COUNTERFEIT_SELLER.to_string()]),
        logistics: LogisticsEntry {
            carrier: "paper-carrier".to_string(),
            warehouse: "paper-warehouse".to_string(),
            consolidate_with: None,
            insurance: None,
            decline_reason: Some("goods under the insured threshold".to_string()),
            delivery_risk: dec!("0.01"),
        },
        resale: ResaleEntry {
            floor_per_unit: dec!("1"),
            markdown_after_days: 3,
            markdown_rate: dec!("0.05"),
        },
    })
}

/// R12-COMMERCE-5090 to -5097 through the cycle: candidates are scouted and
/// resolved in DISCOVER; in SIMULATE a viable one is assessed, bought on
/// paper through the executor's controls, received into the ledger with all
/// twelve attributes, routed and listed, and sold and settled once its
/// venue's time to sale has passed.
///
/// Mutation: delete `self.breadth_simulate(...)` from `run_cycle` and
/// nothing is assessed; skip the `plane_apply` in `work_candidate` and the
/// ledger holds no unit.
#[test]
fn commerce_research_scouts_buys_receives_lists_and_sells_on_paper_through_the_cycle() -> Result<()>
{
    let mut platform = platform()?;
    platform.install_breadth(
        &BreadthPlan {
            commerce: Some(commerce_plan()?),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    let mut at = start();
    for _ in 0..6 {
        platform.run_cycle(at);
        at = at.saturating_add(Duration::from_hours(1));
    }
    let mut runs = Vec::new();
    for event in platform.event_log().by_topic(Topic::ScenarioEvaluated) {
        if let Ok(decoded) = StreamEnvelope::from_frame(event)?.decode::<CommerceSimulated>() {
            runs.push(decoded.body);
        }
    }
    assert_eq!(runs.len(), 6, "one commerce record per SIMULATE");
    let assessed: usize = runs.iter().map(|run| run.assessed.len()).sum();
    assert!(assessed > 0, "nothing was assessed: {runs:?}");
    let research = platform.commerce_research().expect("installed");
    // Premise: a viable candidate was bought, so the checks below have a
    // unit to look at.
    let bought = research.executor().records();
    assert!(!bought.is_empty(), "{runs:?}");
    let unit = research
        .ledger()
        .units()
        .keys()
        .next()
        .cloned()
        .or_else(|| {
            runs.iter()
                .flat_map(|run| run.listed.iter())
                .map(|listed| listed.unit.clone())
                .next()
        })
        .expect("a unit was received");
    let attributes = research.attributes(&unit).expect("the unit has its record");
    for name in qip_financial::commerce_resale::UNIT_ATTRIBUTES {
        assert!(attributes.get(name).is_some(), "{name} missing");
    }
    assert!(
        runs.iter().any(|run| !run.listed.is_empty()),
        "nothing was listed: {runs:?}"
    );
    assert!(
        !research.cycles().is_empty(),
        "no unit completed a purchase-to-sale cycle: {runs:?}"
    );
    assert!(runs.iter().all(|run| !run.decision_grade));
    Ok(())
}
/// R12-EVENT-5087: an event market is created only where the access book
/// records a creation permission from its jurisdiction, before the factory
/// touches the venue; with the permission recorded the same plan installs.
///
/// Mutation: make `AccessBook::admit_creation` admit on an empty book (or
/// skip the event-contract loop in `install_breadth`) and the refused plan
/// installs.
#[test]
fn an_event_market_is_created_only_where_the_access_book_permits_creation() -> Result<()> {
    let plan_with = |creation: BTreeSet<Jurisdiction>| -> Result<BreadthPlan> {
        let mut factory = factory_plan("obj-NEVER-OBSERVED");
        factory.markets[0].event_contract = true;
        let mut events = event_plan(eligible()?, VenueKind::Event)?;
        events.policies.push(VenuePolicy {
            venue: VenueId::new(VENUE),
            kind: VenueKind::Event,
            creation,
            trading: BTreeSet::new(),
            products: BTreeSet::new(),
            minimum_age: None,
        });
        Ok(BreadthPlan {
            market_factory: Some(factory),
            event_markets: Some(events),
            ..BreadthPlan::default()
        })
    };

    let mut refused = platform()?;
    let error = refused
        .install_breadth(&plan_with(BTreeSet::new())?, start())
        .expect_err("an event market was created with no creation permission");
    assert!(
        error.message().contains("event market obj-CREATED refused"),
        "{}",
        error.message()
    );
    assert!(refused.market_factory().is_none());
    assert!(factory_records(&refused)?.is_empty());

    let mut admitted = platform()?;
    admitted.install_breadth(
        &plan_with(BTreeSet::from([Jurisdiction::new("GB")?]))?,
        start(),
    )?;
    assert_eq!(
        admitted
            .market_factory()
            .and_then(|stage| stage.factory().market(PRODUCT))
            .map(|market| market.stage),
        Some(MarketStage::Seeded)
    );
    Ok(())
}

/// R12-EVENT-5084: the belief is estimated from the books, not read off
/// them. Three books whose implied midpoints sum to 1.2 — a 20% overround —
/// must become a distribution proportional to those midpoints, so 0.5, 0.4
/// and 0.3 become 5/12, 4/12 and 3/12.
///
/// The cycle tests above cannot see this: the synthetic venue's midpoints
/// already sum to within a rounding of one, so a plane that skipped the
/// division and let the rounding residual absorb the overround priced them
/// identically. That mutation survived them; this test is why it no longer
/// does. Mutation: divide by one instead of the total in `normalise` and the
/// top outcome takes the whole overround (0.3 instead of 5/12).
#[test]
fn an_overround_book_set_is_normalised_into_a_distribution_proportional_to_its_midpoints()
-> Result<()> {
    let market = qip_prediction::demo_market(
        VenueId::new(EVENT_VENUE),
        start().saturating_add(Duration::from_days(30)),
        qip_prediction::FeeSchedule::new(0, 0, 0)?,
    )?;
    let quotes = [
        ("cut", "0.45", "0.55"),
        ("hold", "0.35", "0.45"),
        ("hike", "0.25", "0.35"),
    ];
    let books: Vec<(qip_prediction::OutcomeId, qip_market::book::OrderBook)> = quotes
        .iter()
        .map(|(id, bid, ask)| {
            let price = |text: &str| Decimal::parse(text).expect("a literal price");
            (
                qip_prediction::OutcomeId::new(*id),
                qip_market::book::OrderBook::from_levels(
                    qip_core::ObjectId::from_string(format!("SYNTH-PREDICT-{}", id.to_uppercase())),
                    EVENT_VENUE,
                    start(),
                    vec![qip_market::book::BookLevel::new(price(bid), dec!("10"))],
                    vec![qip_market::book::BookLevel::new(price(ask), dec!("10"))],
                ),
            )
        })
        .collect();
    let borrowed: Vec<_> = books.iter().map(|(id, book)| (id.clone(), book)).collect();
    // Premise: the books really are overround, or proportionality is moot.
    let midpoints: Decimal = quotes
        .iter()
        .map(|(_, bid, ask)| {
            (Decimal::parse(bid).expect("bid") + Decimal::parse(ask).expect("ask")) / dec!("2")
        })
        .sum();
    assert_eq!(midpoints, Decimal::parse("1.2").expect("literal"));

    let (_, valuation) = EventMarketPlane::price(&market, &borrowed, start())?;
    let total: Decimal = valuation.fair_values.values().copied().sum();
    assert_eq!(total, Decimal::ONE);
    let tolerance = Decimal::parse("0.000001").expect("literal");
    for (id, twelfths) in [("cut", 5), ("hold", 4), ("hike", 3)] {
        let fair = valuation
            .fair_values
            .get(&qip_prediction::OutcomeId::new(id))
            .copied()
            .expect("every outcome is priced");
        let expected = Decimal::from_int(twelfths) / Decimal::from_int(12);
        assert!(
            (fair - expected).abs() < tolerance,
            "{id}: {fair} is not {twelfths}/12 of the unit payoff"
        );
    }
    Ok(())
}
