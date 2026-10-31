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
use qip_kernel::breadth::{BreadthPlan, EventMarketCreationAdmitted, EventMarketPlan};
use qip_kernel::commerce_research::{
    AccountEntry, CommercePlan, CommerceSimulated, LogisticsEntry, ResaleEntry,
    SYNTHETIC_COUNTERFEIT_SELLER, SyntheticCommerceFeed,
};
use qip_kernel::event_markets::{EventMarketPlane, EventMarketsDiscovered, EventMarketsSimulated};
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
const EVENT_MARKET: &str = "SYNTH-PREDICT-MARKET";
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

/// R12-COVERAGE-5107: installation is all or nothing on the shared venue.
/// The plan's first market is fully permitted and installs on its own; its
/// second is fully permitted and listed, and then refused at its seed for
/// inventory that cannot cover the ask side. The failure this prevents is
/// the first market's ladder left resting on the desk broker's venue — and
/// the second's empty book beside it — for a plan the caller was told was
/// refused, under a factory that was dropped with the refusal and so could
/// never review or withdraw either.
///
/// Mutation: drop the `venue.withdraw_book(product)` loop from
/// `FactoryStage::install` and the first market's book still rests.
#[test]
fn a_plan_refused_part_way_through_leaves_nothing_listed_on_the_venue() -> Result<()> {
    const SECOND: &str = "obj-SECOND";
    // Premise: the first market on its own installs and rests a book.
    let mut premise = platform()?;
    premise.install_breadth(
        &BreadthPlan {
            market_factory: Some(factory_plan("obj-NEVER-OBSERVED")),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    assert!(premise.listed_book(PRODUCT).is_some());

    let mut plan = factory_plan("obj-NEVER-OBSERVED");
    let seconded: Vec<PermissionEntry> = plan
        .permissions
        .iter()
        .map(|entry| PermissionEntry {
            product: SECOND.to_string(),
            ..entry.clone()
        })
        .collect();
    plan.permissions.extend(seconded);
    let mut second = plan.markets[0].clone();
    second.product = SECOND.to_string();
    second.inventory = dec!("1");
    plan.markets.push(second);

    let mut platform = platform()?;
    let refused = platform
        .install_breadth(
            &BreadthPlan {
                market_factory: Some(plan),
                ..BreadthPlan::default()
            },
            start(),
        )
        .expect_err("a plan whose second market cannot be seeded installed");
    assert!(
        refused.message().contains("market obj-SECOND")
            && refused.message().contains("cannot cover"),
        "refused for another reason: {}",
        refused.message()
    );
    assert!(
        platform.listed_book(PRODUCT).is_none(),
        "the first market's book still rests on the venue after the plan was refused"
    );
    assert!(
        platform.listed_book(SECOND).is_none(),
        "the second market's empty book still rests on the venue"
    );
    assert!(platform.market_factory().is_none());
    assert!(factory_records(&platform)?.is_empty());
    Ok(())
}

/// R12-COVERAGE-5107: installation never replaces, and never withdraws, a
/// book it did not put on the shared venue. `SimulatedBroker::list_book`
/// replaces whatever rests under the product, so without the check a plan
/// naming a product that already rests would overwrite that ladder with an
/// empty book, and a refusal later in the plan would then withdraw a book
/// this installation never owned. The plan here is fully permitted and its
/// second market is the product already resting.
///
/// Mutation: make the `venue.listed_book(..).is_some()` check in
/// `FactoryStage::take_through` never fire and the plan installs over the
/// resting book (found by the round-5 skeptic: until then unpinned).
#[test]
fn a_plan_naming_a_product_that_already_rests_on_the_venue_is_refused_and_leaves_that_book_alone()
-> Result<()> {
    use qip_execution_engine::broker::{ListingVenue, SimulatedBroker, SimulationSettings};
    use qip_execution_engine::creation::{CreationAction, CreationAuthority, CreationRequest};
    use qip_execution_engine::market_creation::PricingRule;
    use qip_kernel::market_factory_stage::FactoryStage;
    const RESTING: &str = "obj-RESTING";

    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
    let rule = PricingRule::new(
        dec!("100"),
        dec!("0.5"),
        dec!("0.25"),
        dec!("10"),
        3,
        dec!("0.05"),
    )?;
    // The foreign book reaches the venue the only way a resting book can: listed
    // empty under a listing admission, then seeded under its own seeding
    // admission (R12-EXEC-5079 refuses a listing that arrives with levels).
    let foreign = rule.book(RESTING, VENUE, start())?;
    let mut register = CreationPermissions::new();
    for action in [CreationAction::List, CreationAction::Seed] {
        let request = CreationRequest::new(action, VENUE, RESTING, "GB");
        for authority in CreationAuthority::ALL {
            register.record(authority, &request, "resting before the plan")?;
        }
    }
    broker.list_book(
        qip_market::book::OrderBook::new(qip_core::ObjectId::from_string(RESTING), VENUE, start()),
        &register.admit(&CreationRequest::new(
            CreationAction::List,
            VENUE,
            RESTING,
            "GB",
        ))?,
    )?;
    broker.list_book(
        foreign,
        &register.admit(&CreationRequest::new(
            CreationAction::Seed,
            VENUE,
            RESTING,
            "GB",
        ))?,
    )?;
    let depth = broker
        .listed_book(RESTING)
        .map(|book| book.total_depth())
        .expect("premise: the foreign book rests");
    assert!(depth.is_positive(), "premise: the foreign book has depth");

    let mut plan = factory_plan("obj-NEVER-OBSERVED");
    let resting: Vec<PermissionEntry> = plan
        .permissions
        .iter()
        .map(|entry| PermissionEntry {
            product: RESTING.to_string(),
            ..entry.clone()
        })
        .collect();
    plan.permissions.extend(resting);
    let mut second = plan.markets[0].clone();
    second.product = RESTING.to_string();
    plan.markets.push(second);

    let mut ladders = |product: &str, venue: &str, rule: &PricingRule| {
        let mut manager = qip_execution_engine::oms::OrderManager::new(
            qip_risk_engine::pretrade::PreTradeChecker::new(LimitSet::new("breadth-test")),
        );
        let state = qip_risk::limits::RiskState {
            equity: dec!("1000000"),
            cash: dec!("1000000"),
            ..qip_risk::limits::RiskState::default()
        };
        manager.admit_ladder(
            product,
            venue,
            rule,
            "market-factory",
            &qip_risk_engine::autonomy::AutonomyController::new(),
            &state,
            start(),
        )
    };
    let refused = FactoryStage::install(&plan, &mut broker, &mut ladders, start())
        .expect_err("a plan was installed over a book already resting on the venue");
    assert!(
        refused
            .message()
            .contains("already rests a book for obj-RESTING"),
        "refused for another reason: {}",
        refused.message()
    );
    assert_eq!(
        broker.listed_book(RESTING).map(|book| book.total_depth()),
        Some(depth),
        "the book that rested before the plan was replaced or withdrawn"
    );
    assert!(
        broker.listed_book(PRODUCT).is_none(),
        "the plan's first market was left on the venue after the refusal"
    );
    Ok(())
}

/// The event plane's own market and the instrument each of its outcomes
/// trades as: everything the synthetic venue brings into existence, so
/// everything that needs a creation admission.
fn event_instruments() -> Vec<&'static str> {
    std::iter::once(EVENT_MARKET)
        .chain(EVENT_PRODUCTS)
        .collect()
}

/// `create`, `list` and `seed` from all three authorities for every event
/// instrument, at the event venue in GB — the register a plan that installs
/// the event plane, takes its listing in and rests paper quotes on it has to
/// record (R12-COVERAGE-5107: each act asks for the action it performs).
fn event_permissions() -> Vec<PermissionEntry> {
    let mut permissions = Vec::new();
    for product in event_instruments() {
        for action in ["create", "list", "seed"] {
            for authority in ["venue", "product", "jurisdiction"] {
                permissions.push(PermissionEntry {
                    authority: authority.to_string(),
                    action: action.to_string(),
                    venue: EVENT_VENUE.to_string(),
                    product: product.to_string(),
                    jurisdiction: "GB".to_string(),
                    reference: format!("{authority} approval for {product}"),
                });
            }
        }
    }
    permissions
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
        jurisdiction: "GB".to_string(),
        permissions: event_permissions(),
    })
}

fn event_creations(platform: &Platform) -> Result<Vec<EventMarketCreationAdmitted>> {
    let mut records = Vec::new();
    for event in platform.event_log().by_topic(Topic::ComplianceEvaluated) {
        if let Ok(decoded) =
            StreamEnvelope::from_frame(event)?.decode::<EventMarketCreationAdmitted>()
        {
            records.push(decoded.body);
        }
    }
    Ok(records)
}

/// R12-COVERAGE-5107, the event plane's half: its own market and the three
/// outcome instruments its venue quotes are created only under a recorded
/// `create` permission from the venue, the product rules and the
/// jurisdiction, each. A plan missing any one is refused whole, and creates
/// nothing — no venue, no market, no instrument, no record, and not the
/// Market Factory's books either.
///
/// Until 2026-10-24 the event plane built `SYNTH-PREDICT-MARKET` and its
/// instruments with no admission of any kind: the `event_plan` fixture here
/// recorded no permission and installed. And the factory was installed
/// first, so a plan refused for its event section would have left the
/// factory's ladder resting on the desk's venue; the last case pairs the
/// refused section with a fully permitted factory to catch that.
///
/// Mutations: (1) swallow the refusal — `admit_event_market(..)` with
/// `.unwrap_or_default()` for `?` in `open_event_plane` — and every refused
/// plan installs; (2) ask the register about the market id alone, not its
/// outcome instruments, and the plan missing one outcome's permission
/// installs; (3) build the factory before the event plane in
/// `install_breadth` and the refused plan leaves the factory's book on the
/// venue.
#[test]
fn an_event_plan_without_every_creation_permission_is_refused_whole_and_creates_nothing()
-> Result<()> {
    // Premise: with the full register the same plan installs and the
    // factory's book rests on the desk's venue, so each refusal below is the
    // missing permission and each absent book is one the refusal prevented.
    let mut premise = platform()?;
    premise.install_breadth(
        &BreadthPlan {
            market_factory: Some(factory_plan("obj-NEVER-OBSERVED")),
            event_markets: Some(event_plan(eligible()?, VenueKind::Event)?),
            ..BreadthPlan::default()
        },
        start(),
    )?;
    assert!(premise.event_markets().is_some());
    assert!(premise.listed_book(PRODUCT).is_some());

    let without = |authority: &str, product: &str| -> Result<EventMarketPlan> {
        let mut plan = event_plan(eligible()?, VenueKind::Event)?;
        plan.permissions
            .retain(|entry| !(entry.authority == authority && entry.product == product));
        Ok(plan)
    };
    let mut nothing = event_plan(eligible()?, VenueKind::Event)?;
    nothing.permissions.clear();
    let mut elsewhere = event_plan(eligible()?, VenueKind::Event)?;
    elsewhere.jurisdiction = "US".to_string();
    let cases = [
        ("no permission recorded", nothing, EVENT_MARKET),
        (
            "the market lacks its product approval",
            without("product", EVENT_MARKET)?,
            EVENT_MARKET,
        ),
        (
            "one outcome instrument lacks its legal opinion",
            without("jurisdiction", "SYNTH-PREDICT-HIKE")?,
            "SYNTH-PREDICT-HIKE",
        ),
        (
            "every permission is for another jurisdiction",
            elsewhere,
            "SYNTH-PREDICT-CUT",
        ),
    ];
    for (why, events, refused_instrument) in cases {
        for with_factory in [false, true] {
            let mut platform = platform()?;
            let refused = platform
                .install_breadth(
                    &BreadthPlan {
                        market_factory: with_factory.then(|| factory_plan("obj-NEVER-OBSERVED")),
                        event_markets: Some(events.clone()),
                        ..BreadthPlan::default()
                    },
                    start(),
                )
                .expect_err(why);
            let message = refused.message();
            assert!(
                message.contains(&format!("{refused_instrument}: no market may be created")),
                "{why}: the refusal does not name {refused_instrument}: {message}"
            );
            assert!(
                message.contains("No venue, market or instrument was created"),
                "{why}: {message}"
            );
            assert!(platform.event_markets().is_none(), "{why}");
            assert!(platform.market_factory().is_none(), "{why}");
            assert!(
                platform.listed_book(PRODUCT).is_none(),
                "{why}: the refused plan left the factory's book on the desk's venue"
            );
            assert!(event_creations(&platform)?.is_empty(), "{why}");
            assert!(factory_records(&platform)?.is_empty(), "{why}");
            // Nothing runs afterwards either: no listing reaches the plane,
            // because there is no plane.
            platform.run_cycle(start());
            assert!(platform.event_markets().is_none(), "{why}");
        }
    }
    Ok(())
}

/// R12-COVERAGE-5107: every instrument the installed event plane lists —
/// the market and each outcome its venue quotes — is in the log with the
/// admission it was created under, and nothing it lists is missing one.
/// The set is read off the plane after a cycle, not off the constants the
/// admission loop reads, so an outcome added to `qip_prediction::demo_market`
/// without its admission fails here rather than trading unadmitted.
///
/// Mutations: (1) delete the loop journaling `created` in `install_breadth`
/// and the log holds no admission; (2) give one of `demo_market`'s outcomes
/// an instrument literal in place of its `DEMO_OUTCOME_INSTRUMENTS` entry
/// and the plane lists an instrument with no admission.
#[test]
fn every_instrument_the_event_plane_lists_is_journaled_with_the_admission_it_was_created_under()
-> Result<()> {
    let (platform, _) = run_event_cycle(eligible()?, VenueKind::Event)?;
    let plane = platform.event_markets().expect("installed");
    let market = plane.markets().next().expect("the plane listed its market");
    let listed: BTreeSet<String> = std::iter::once(market.market_id.as_str().to_string())
        .chain(
            market
                .outcomes()
                .iter()
                .map(|outcome| outcome.object_id.as_str().to_string()),
        )
        .collect();
    // Premise: the plane lists the market and three outcome instruments, so
    // an empty or partial log below is a missing admission.
    assert_eq!(listed.len(), 4, "{listed:?}");

    let created = event_creations(&platform)?;
    let admitted: BTreeSet<String> = created.iter().map(|r| r.product.clone()).collect();
    assert_eq!(admitted, listed, "{created:?}");
    assert_eq!(created.len(), listed.len(), "one admission per instrument");
    for record in &created {
        assert_eq!(record.venue, EVENT_VENUE);
        assert_eq!(record.jurisdiction, "GB");
        assert_eq!(
            record.approvals,
            ["venue", "product", "jurisdiction"]
                .map(|authority| format!(
                    "{authority}: {authority} approval for {}",
                    record.product
                ))
                .to_vec(),
            "the record cites the evidence each authority's permission rests on"
        );
    }
    assert!(platform.event_log().verify_chain().is_ok());
    Ok(())
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
        CreationPermissions::new(),
        "GB",
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
    // R12-LEDGER-5026: every SIMULATE pass reconciles the ledger against the
    // marketplace's own record. The marketplace booked every settled sale,
    // and an agreeing record leaves no break and no reported problem.
    // Mutation: drop the reconcile call from `CommerceResearch::simulate`
    // and the pass count reads zero.
    let plane = research.plane();
    assert_eq!(
        plane.reconciliations(),
        6,
        "one reconciliation per SIMULATE"
    );
    assert_eq!(
        plane.marketplace_statement().lines.len(),
        research.cycles().len(),
        "the marketplace booked every sale the ledger settled"
    );
    assert!(plane.breaks().is_empty(), "{:?}", plane.breaks());
    assert!(
        runs.iter()
            .flat_map(|run| run.problems.iter())
            .all(|problem| !problem.starts_with("reconciliation break")),
        "{runs:?}"
    );
    assert!(runs.iter().all(|run| !run.decision_grade));
    Ok(())
}
/// R12-COMMERCE-5090: the tracked condition, location and inventory state of
/// a unit are the ledger's, after the unit has moved. They were the literals
/// "new", the warehouse and "held", written once at receipt, so a unit sold
/// and settled (or returned open-box) was still described as new and held.
///
/// Mutation: delete the `refresh_attributes` call after `take_return_owed` -
/// a sold unit still reads "held" and the agreement assertion fails.
#[test]
fn a_units_tracked_state_follows_the_ledger_after_it_is_sold() -> Result<()> {
    use qip_financial::commerce::UnitState;
    use qip_financial::commerce_resale::Attr;

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
    let research = platform.commerce_research().expect("installed");
    let units = research.ledger().units();
    let sold: Vec<&String> = units
        .iter()
        .filter(|(_, record)| matches!(record.state, UnitState::Sold { .. }))
        .map(|(unit, _)| unit)
        .collect();
    assert!(!sold.is_empty(), "premise: a unit was sold in six cycles");
    for (unit, record) in units {
        let attributes = research.attributes(unit).expect("a record for every unit");
        let state = match &record.state {
            UnitState::Held => "held",
            UnitState::Reserved { .. } => "reserved",
            UnitState::Sold { .. } => "sold",
        };
        assert_eq!(
            attributes.get("inventory_state"),
            Some(&Attr::Value(state.to_string())),
            "{unit} is {state} in the ledger"
        );
        assert_eq!(
            attributes.get("location"),
            Some(&Attr::Value(record.location.clone())),
            "{unit}'s location"
        );
        assert_eq!(
            attributes.get("condition"),
            Some(&Attr::Value(
                match record.condition {
                    qip_financial::commerce::Condition::New => "new",
                    qip_financial::commerce::Condition::OpenBox => "open_box",
                    qip_financial::commerce::Condition::Used => "used",
                    qip_financial::commerce::Condition::Damaged => "damaged",
                }
                .to_string()
            )),
            "{unit}'s condition"
        );
    }
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

/// The event plane's quotes and listings after one installed cycle, under
/// the full register less every `(authority, action, product)` in `revoked`.
fn event_cycle_without(
    revoked: &[(&str, &str, &str)],
) -> Result<(Platform, EventMarketsSimulated)> {
    let mut plan = event_plan(eligible()?, VenueKind::Event)?;
    plan.permissions.retain(|entry| {
        !revoked.contains(&(
            entry.authority.as_str(),
            entry.action.as_str(),
            entry.product.as_str(),
        ))
    });
    let mut platform = platform()?;
    platform.install_breadth(
        &BreadthPlan {
            event_markets: Some(plan),
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

/// R12-COVERAGE-5107: each act the event plane performs asks the creation
/// register for the action it is. Until 2026-10-24 the plane asked only for
/// `create`, once, at install, and then took the venue's listing in and
/// rested paper quotes on every outcome under no permission to list or to
/// seed — so a register that permitted a market to be built had, without
/// anybody recording it, permitted it to be listed and quoted.
///
/// With `create` alone the plan installs (the market is built) and nothing
/// is taken in; with `create` and `list` the market is listed and priced and
/// no quote rests; one outcome missing its `seed` loses that outcome's quote
/// alone.
///
/// Mutations: (1) make the `list` refusal in `EventMarketPlane::discover`'s
/// `MarketListed` arm never fire (`if false`) and the create-only run lists
/// the market; (2) admit the quote under `CreationAction::List` instead of
/// `Seed` in `simulate` and the no-seed run quotes.
#[test]
fn the_event_plane_lists_only_under_list_and_quotes_only_under_seed() -> Result<()> {
    let every = |action: &'static str| -> Vec<(&'static str, &'static str, &'static str)> {
        event_instruments()
            .into_iter()
            .flat_map(|product| {
                ["venue", "product", "jurisdiction"].map(|authority| (authority, action, product))
            })
            .collect()
    };
    // Premise: the full register lists, prices and quotes every outcome.
    let (full, quoted) = event_cycle_without(&[])?;
    assert_eq!(
        full.event_markets().map(|plane| plane.markets().count()),
        Some(1)
    );
    assert_eq!(quoted.quotes.len(), 3, "{quoted:?}");

    let mut create_only = every("list");
    create_only.extend(every("seed"));
    let (platform, run) = event_cycle_without(&create_only)?;
    let plane = platform
        .event_markets()
        .expect("create alone installs the plane");
    assert_eq!(
        plane.markets().count(),
        0,
        "a market was listed under create alone"
    );
    assert!(
        run.valuations.is_empty() && run.quotes.is_empty(),
        "{run:?}"
    );

    let (platform, run) = event_cycle_without(&every("seed"))?;
    assert_eq!(
        platform
            .event_markets()
            .map(|plane| plane.markets().count()),
        Some(1),
        "premise: create and list take the market in"
    );
    assert_eq!(
        run.valuations.len(),
        1,
        "the listed market is priced: {run:?}"
    );
    assert!(
        run.quotes.is_empty(),
        "a quote rested with no permission to seed: {run:?}"
    );
    assert_eq!(
        run.refusals
            .iter()
            .filter(
                |reason| reason.contains("to seed") && reason.contains("no market may be created")
            )
            .count(),
        3,
        "each outcome's quote is refused for its seed permission: {:?}",
        run.refusals
    );

    let (_, run) = event_cycle_without(&[("jurisdiction", "seed", "SYNTH-PREDICT-HIKE")])?;
    let outcomes: BTreeSet<String> = run
        .quotes
        .iter()
        .map(|quote| quote.outcome.to_string())
        .collect();
    assert_eq!(
        outcomes,
        BTreeSet::from(["cut".to_string(), "hold".to_string()]),
        "only the outcome missing its seed permission lost its quote"
    );
    Ok(())
}

/// R12-COVERAGE-5107: a permission revoked from a running event plane stops
/// it. Until 2026-10-24 the event plane's register was read at install only,
/// and `PermissionWatch` re-read the factory's alone, so an operator who
/// deleted the legal opinion an event market was listed under left it
/// priced, quoted and traded for as long as the process ran.
///
/// Revoking one outcome instrument's `list` permission, or its `create`
/// permission, withdraws the whole market at the next cycle — a complete set with a leg the plane may not
/// hold is not a market it may make — and the withdrawal is journaled with
/// the refusal. Revoking everything does the same through the fail-closed
/// path.
///
/// Mutations: (1) make `withdraw_lapsed` keep every market (`if true {
/// continue; }`) and the revoked market is still quoted; (1a) replace its
/// `create` refusals with an empty list and the create-revoked case is
/// still quoted (found by the round-5 skeptic: until then only the `list`
/// half was pinned); (2) make
/// `revise_event_market_permissions` skip `set_creation_permissions` and
/// the revocation never reaches the plane.
#[test]
fn a_revoked_event_permission_withdraws_the_market_at_the_next_cycle() -> Result<()> {
    // `Some(action)` revokes one authority's permission for that action on
    // one outcome instrument; `None` revokes the whole register.
    for revoked in [Some("list"), Some("create"), None] {
        let (mut platform, first) = run_event_cycle(eligible()?, VenueKind::Event)?;
        // Premise: the fully permitted market is quoted on every outcome.
        assert_eq!(first.quotes.len(), 3, "{first:?}");
        if let Some(action) = revoked {
            let entries: Vec<PermissionEntry> = event_permissions()
                .into_iter()
                .filter(|entry| {
                    !(entry.action == action
                        && entry.authority == "jurisdiction"
                        && entry.product == "SYNTH-PREDICT-CUT")
                })
                .collect();
            assert_eq!(
                platform.revise_event_market_permissions(&entries)?,
                event_permissions().len() - 1
            );
        } else {
            platform.revoke_event_market_permissions();
        }
        let later = start().saturating_add(Duration::from_hours(1));
        platform.run_cycle(later);
        let after = simulated_event_records(&platform)?
            .pop()
            .expect("SIMULATE journaled the event markets");
        assert!(
            after.quotes.is_empty() && after.fills.is_empty() && after.valuations.is_empty(),
            "revoked={revoked:?}: a revoked market was still priced or quoted: {after:?}"
        );
        let plane = platform.event_markets().expect("installed");
        assert!(
            plane.is_withdrawn(&qip_core::ObjectId::from_string(EVENT_MARKET)),
            "revoked={revoked:?}: the market was not withdrawn"
        );
        let discovered: Vec<EventMarketsDiscovered> = platform
            .event_log()
            .by_topic(Topic::OpportunityDetected)
            .into_iter()
            .filter_map(|event| {
                StreamEnvelope::from_frame(event)
                    .ok()?
                    .decode::<EventMarketsDiscovered>()
                    .ok()
                    .map(|decoded| decoded.body)
            })
            .collect();
        let withdrawn: Vec<&String> = discovered
            .iter()
            .flat_map(|record| record.withdrawn.iter())
            .collect();
        assert_eq!(withdrawn.len(), 1, "revoked={revoked:?}: {withdrawn:?}");
        let (cut, action) = match revoked {
            Some(action) => ("SYNTH-PREDICT-CUT", action),
            None => ("SYNTH-PREDICT-MARKET", "create"),
        };
        assert!(
            withdrawn[0].contains(EVENT_MARKET)
                && withdrawn[0].contains(&format!(
                    "{cut}: no market may be created here: to {action} {cut}"
                )),
            "revoked={revoked:?}: the withdrawal does not carry the refusal: {}",
            withdrawn[0]
        );
    }
    Ok(())
}

/// R12-ASSET-4073: each unit the simulated merchant delivers becomes a
/// RealAsset position record carrying the purchase's landed cost and the
/// unit's state as the inventory ledger holds it, and the record stays in
/// the commerce research output: the desk's book holds none of it.
///
/// Mutation: in `CommerceResearch::position_record`, set the inventory
/// state from a literal `UnitState::Sold` instead of the ledger read, and
/// the state assertion fails.
#[test]
fn a_received_paper_unit_is_a_real_asset_position_carrying_landed_cost_and_ledger_inventory_and_the_book_is_unchanged()
-> Result<()> {
    use qip_financial::asset_class::AssetClass;
    use qip_financial::commerce::UnitState;
    let mut platform = platform()?;
    let book_before = platform.positions_at_cost();
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
    let received: Vec<String> = runs
        .iter()
        .flat_map(|run| run.purchases.iter())
        .flat_map(|purchase| purchase.units.iter().cloned())
        .collect();
    assert!(
        !received.is_empty(),
        "premise: a unit was received: {runs:?}"
    );
    let positions: Vec<&qip_financial::position_record::PositionRecord> =
        runs.iter().flat_map(|run| run.positions.iter()).collect();
    assert_eq!(
        positions
            .iter()
            .map(|p| p.asset().to_string())
            .collect::<Vec<_>>(),
        received,
        "one position record per received unit"
    );
    // The journal carries each assessment's duty apart (R12-COMMERCE-1093),
    // so a record's landed cost is matched to its purchase by that figure.
    let duties: Vec<qip_core::Decimal> = runs
        .iter()
        .flat_map(|run| run.assessed.iter())
        .filter(|assessed| assessed.viable)
        .map(|assessed| assessed.duty)
        .collect();
    for record in &positions {
        assert_eq!(record.class(), AssetClass::RealAsset);
        let economics = record
            .product_economics()
            .expect("a physical unit carries its landed cost");
        assert!(
            economics.goods.is_positive() && duties.contains(&economics.duty),
            "the landed cost is not the purchase's own: {economics:?} against duties {duties:?}"
        );
        assert_eq!(
            record.inventory_state(),
            Some(&UnitState::Held),
            "a unit is held in the ledger at receipt"
        );
    }
    assert_eq!(
        platform.positions_at_cost(),
        book_before,
        "a commerce research unit reached the desk's book"
    );
    assert!(
        platform
            .positions_at_cost()
            .keys()
            .all(|instrument| !received.contains(instrument)),
        "a received unit is a book position"
    );
    Ok(())
}
