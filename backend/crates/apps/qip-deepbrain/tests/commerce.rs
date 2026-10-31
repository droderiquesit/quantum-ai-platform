//! Physical commerce research from the deep brain's breadth plan to the
//! inventory ledger (R12-COMMERCE-5091 to -5097, -1087 to -1094, -1171).
//!
//! `main` reads `QIP_DEEPBRAIN_BREADTH_PLAN` and hands it to
//! `qip_deepbrain::breadth::install`; unset is no commerce at all. These
//! tests write the plan as the JSON file a deployment would, install it
//! through that same function with a lookup standing in for the environment,
//! and run real cycles — so every assertion is about what the binary's path
//! does to the ledger, not what the library can do when called by hand. The
//! kernel's own `tests/breadth.rs` calls `Platform::install_breadth`
//! directly and would pass with this file's path broken.
//!
//! Everything here is paper: the executor holds the simulated merchant by
//! concrete type and the feed is synthetic and research-only, so no record
//! is decision-grade and no figure reaches the desk's sizing.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::Result;
use qip_core::{Context, Decimal, Duration, Timestamp, dec};
use qip_deepbrain::breadth::{PLAN_VARIABLE, install};
use qip_events::Topic;
use qip_financial::commerce::{Condition, Ledger, LedgerEvent, OpportunityKind, UnitState};
use qip_financial::physical::{
    Customs, Leg, LogisticsTerms, MarketplaceFees, Returns, Spoilage, TransportMode,
};
use qip_financial::universe::Universe;
use qip_kernel::breadth::BreadthPlan;
use qip_kernel::commerce_research::{
    AccountEntry, CommerceDiscovered, CommercePlan, CommerceSimulated, LogisticsEntry, ResaleEntry,
    SYNTHETIC_COUNTERFEIT_SELLER, SyntheticCommerceFeed,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_streaming::envelope::StreamEnvelope;

fn ad_valorem() -> Decimal {
    dec!("0.05")
}
const WAREHOUSE: &str = "paper-warehouse";
const OWNER: &str = "desk";

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

/// The plan a desk would write: synthetic source, one road leg, a 5%
/// marketplace fee, every synthetic venue on the account.
fn commerce_plan() -> Result<CommercePlan> {
    Ok(CommercePlan {
        source: "synthetic".to_string(),
        owner: OWNER.to_string(),
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
                ad_valorem: ad_valorem(),
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
            warehouse: WAREHOUSE.to_string(),
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

/// Write `plan` as the JSON file `QIP_DEEPBRAIN_BREADTH_PLAN` names, install
/// it through `breadth::install`, and run `cycles` hourly cycles.
fn run(name: &str, plan: CommercePlan, cycles: usize) -> Result<Platform> {
    let text = serde_json::to_string(&BreadthPlan {
        commerce: Some(plan),
        ..BreadthPlan::default()
    })
    .expect("a plan serialises");
    let path = std::env::temp_dir().join(format!(
        "qip-deepbrain-commerce-{}-{name}.json",
        std::process::id()
    ));
    std::fs::write(&path, text).expect("the plan file is writable");
    let environment = BTreeMap::from([(
        PLAN_VARIABLE.to_string(),
        path.to_string_lossy().into_owned(),
    )]);
    let mut platform = platform()?;
    let lines = install(
        &mut platform,
        &|name| environment.get(name).cloned(),
        start(),
    )?
    .expect("a plan was set");
    assert_eq!(lines.len(), 1, "one section, one banner line: {lines:?}");
    let mut at = start();
    for _ in 0..cycles {
        platform.run_cycle(at);
        at = at.saturating_add(Duration::from_hours(1));
    }
    Ok(platform)
}

fn simulated(platform: &Platform) -> Result<Vec<CommerceSimulated>> {
    let mut out = Vec::new();
    for event in platform.event_log().by_topic(Topic::ScenarioEvaluated) {
        if let Ok(decoded) = StreamEnvelope::from_frame(event)?.decode::<CommerceSimulated>() {
            out.push(decoded.body);
        }
    }
    Ok(out)
}

fn discovered(platform: &Platform) -> Result<Vec<CommerceDiscovered>> {
    let mut out = Vec::new();
    for event in platform.event_log().by_topic(Topic::OpportunityDetected) {
        if let Ok(decoded) = StreamEnvelope::from_frame(event)?.decode::<CommerceDiscovered>() {
            out.push(decoded.body);
        }
    }
    Ok(out)
}

/// Unset is no commerce: the fail-closed default `main` relies on, because a
/// commerce section commits the platform to paper purchases.
///
/// Mutation: make `breadth::load` install a default commerce plan when the
/// variable is unset and `commerce_research()` is `Some`.
#[test]
fn with_the_breadth_plan_unset_no_commerce_plane_runs() -> Result<()> {
    let mut platform = platform()?;
    let unset: BTreeMap<String, String> = BTreeMap::new();
    assert!(install(&mut platform, &|name| unset.get(name).cloned(), start())?.is_none());
    platform.run_cycle(start());
    assert!(platform.commerce_research().is_none());
    assert!(discovered(&platform)?.is_empty());
    Ok(())
}

/// Discovery, purchasing, logistics, inventory and resale, each from the
/// plan file to a ledger effect (5091-5097, 1087-1091). Product Scout's
/// candidates are journaled; a counterfeit listing is dropped by the SKU
/// resolver and never bought; every assessed candidate carries the six
/// all-in figures; the executor's paper purchases are the only source of
/// units; each unit is received into the event-sourced ledger at the
/// planned warehouse under the planned owner with its logistics recorded,
/// routed to a resale venue, listed, and sold.
///
/// Mutations: delete the `install_breadth` call in `breadth::install` and
/// nothing is discovered; skip `plane_apply` in `work_candidate` and the
/// ledger holds no unit; let a counterfeit identity through in `discover`
/// and a counterfeit seller's listing is queued.
#[test]
fn a_commerce_plan_file_discovers_buys_receives_lists_and_sells_on_paper_into_the_inventory_ledger()
-> Result<()> {
    let platform = run("cycle", commerce_plan()?, 12)?;
    let found = discovered(&platform)?;
    let runs = simulated(&platform)?;
    assert_eq!(found.len(), 12, "one discovery record per DISCOVER");
    assert_eq!(runs.len(), 12, "one simulation record per SIMULATE");

    // 5091 / 1087: Product Scout found candidates, and none is decision-grade.
    let candidates: Vec<_> = found.iter().flat_map(|f| f.candidates.iter()).collect();
    assert!(!candidates.is_empty(), "the scout found nothing: {found:?}");
    assert!(
        candidates
            .iter()
            .any(|c| matches!(c.kind, OpportunityKind::PriceGap)),
        "no price gap among {candidates:?}"
    );
    assert!(
        found
            .iter()
            .all(|f| !f.decision_grade && f.posture == "synthetic")
    );

    // 5092: the resolver dropped counterfeit listings by their finding.
    let counterfeit: Vec<&String> = found.iter().flat_map(|f| f.counterfeit.iter()).collect();
    assert!(
        !counterfeit.is_empty(),
        "the feed's counterfeit seller never showed"
    );

    // 5093: every assessed candidate carries its all-in economics.
    let assessed: Vec<_> = runs.iter().flat_map(|r| r.assessed.iter()).collect();
    assert!(!assessed.is_empty());
    for a in &assessed {
        assert!(a.all_in_cost.is_positive(), "{a:?}");
        assert!(a.expected_resale.is_positive(), "{a:?}");
        assert!(a.capital_lockup.is_positive(), "{a:?}");
        assert!(a.time_to_sale_days > 0, "{a:?}");
        assert_eq!(
            a.viable,
            a.expected_resale > a.all_in_cost + a.inventory_carry
        );
    }

    // 5094 / 1088: purchases went through the executor, inside the ceiling.
    let research = platform.commerce_research().expect("installed");
    let bought = research.executor().records();
    assert!(!bought.is_empty(), "nothing was bought: {runs:?}");
    let notional: Decimal = bought.iter().map(|r| r.notional).sum();
    assert!(notional <= dec!("100000"));

    // 5096 / 1090: every unit in the ledger came from a journaled purchase,
    // sits at the planned warehouse under the planned owner, and the ledger
    // replays from its own events to the same state.
    let ledger = research.ledger();
    let purchased: BTreeSet<&String> = runs
        .iter()
        .flat_map(|r| r.purchases.iter())
        .flat_map(|p| p.units.iter())
        .collect();
    assert!(!ledger.units().is_empty());
    for (unit, record) in ledger.units() {
        assert!(
            purchased.contains(unit),
            "a unit in the ledger was not bought"
        );
        assert_eq!(record.location, WAREHOUSE);
        // Ownership passes to the buyer on sale; until then it is the desk's.
        let owner = match record.state {
            UnitState::Sold { .. } => "paper-buyer",
            _ => OWNER,
        };
        assert_eq!(record.owner, owner, "a unit has the wrong owner");
        assert!(record.cost_basis.is_positive());
    }
    assert_eq!(&Ledger::replay(ledger.events())?, ledger);

    // 5095 / 1089: each unit carries the logistics the plan was built from.
    for unit in ledger.units().keys() {
        let attributes = research
            .attributes(unit)
            .expect("a received unit has its record");
        for name in qip_financial::commerce_resale::UNIT_ATTRIBUTES {
            assert!(attributes.get(name).is_some(), "{unit}: {name} missing");
        }
    }

    // 5097 / 1091: units were listed on a routed venue and sold.
    assert!(runs.iter().any(|r| !r.listed.is_empty()));
    let sold = ledger
        .units()
        .values()
        .filter(|u| matches!(u.state, UnitState::Sold { .. }))
        .count();
    assert!(sold > 0, "no unit was sold");
    assert_eq!(research.cycles().len(), sold);
    assert!(runs.iter().all(|r| !r.decision_grade));
    Ok(())
}

/// The executor's fraud safeguard fires before any merchant call on the
/// binary's path (5094): every venue on the fraud book means every viable
/// candidate is refused and named, and the ledger receives nothing.
///
/// Mutation: drop the fraud-book check from `PurchaseExecutor::purchase`
/// and units are received.
#[test]
fn a_plan_whose_fraud_book_names_every_venue_buys_nothing_and_receives_no_unit() -> Result<()> {
    let mut plan = commerce_plan()?;
    plan.fraud_merchants = SyntheticCommerceFeed::venues().into_iter().collect();
    let platform = run("fraud", plan, 6)?;
    let runs = simulated(&platform)?;
    // Premise: there were viable candidates to refuse.
    assert!(
        runs.iter()
            .flat_map(|r| r.assessed.iter())
            .any(|a| a.viable),
        "{runs:?}"
    );
    assert!(runs.iter().any(|r| !r.refusals.is_empty()), "{runs:?}");
    let research = platform.commerce_research().expect("installed");
    assert!(research.executor().records().is_empty());
    assert!(research.ledger().units().is_empty());
    Ok(())
}

/// Duties and purchase tax are charged inside the all-in cost and journaled
/// apart (1093). The same seeded feed run twice, once duty-free and once at
/// a 10% duty and 20% purchase tax, assesses the same first candidate; the
/// taxed run's duty is the declared value times the rate, its purchase tax
/// the goods times the rate, and its all-in cost is higher by exactly both.
///
/// Mutation: drop `landed.duty` (or `purchase_tax`) from the all-in cost in
/// `FeasibilityInput::assess` and the difference falls short.
#[test]
fn a_duty_and_purchase_tax_in_the_plan_are_charged_into_the_all_in_cost_and_journaled() -> Result<()>
{
    let free = run("duty-free", commerce_plan()?, 1)?;
    let mut taxed_plan = commerce_plan()?;
    taxed_plan.terms.customs.duty_rate = dec!("0.1");
    taxed_plan.purchase_tax_rate = dec!("0.2");
    let taxed = run("duty-taxed", taxed_plan, 1)?;
    let free = simulated(&free)?;
    let taxed = simulated(&taxed)?;
    let free = free[0].assessed.first().expect("a candidate was assessed");
    let taxed = taxed[0].assessed.first().expect("a candidate was assessed");
    assert_eq!((&free.product, &free.venue), (&taxed.product, &taxed.venue));
    assert_eq!(
        (free.duty, free.purchase_tax),
        (Decimal::ZERO, Decimal::ZERO)
    );

    let candidate = discovered(&run("duty-price", commerce_plan()?, 1)?)?[0]
        .candidates
        .iter()
        .find(|c| c.product == taxed.product && c.venue == taxed.venue)
        .cloned()
        .expect("the candidate was discovered");
    let goods = candidate.price * dec!("2");
    assert_eq!(taxed.duty, goods * dec!("0.1"));
    assert_eq!(taxed.purchase_tax, goods * dec!("0.2"));
    assert_eq!(
        taxed.all_in_cost - free.all_in_cost,
        taxed.duty + taxed.purchase_tax
    );
    Ok(())
}

/// A received unit carries its share of the duty and the purchase tax in
/// its cost basis, not only the journaled assessment (R12-COMMERCE-1093).
/// Without this the ledger could book units at their pre-border price while
/// the assessment read correctly, and every margin the plane later realised
/// would be overstated by exactly the border.
///
/// Mutation: drop `self.duty` from `LandedCost::total` and the taxed unit's
/// basis rises over the free one's by the tax alone, not by duty plus tax.
#[test]
fn a_received_units_cost_basis_carries_its_duty_and_purchase_tax() -> Result<()> {
    let mut taxed_plan = commerce_plan()?;
    taxed_plan.terms.customs.duty_rate = dec!("0.1");
    taxed_plan.purchase_tax_rate = dec!("0.2");
    let quantity = taxed_plan.quantity;
    let platform = run("basis-taxed", taxed_plan, 1)?;
    let pass = simulated(&platform)?;
    let purchase = pass[0]
        .purchases
        .first()
        .cloned()
        .expect("one purchase was made");
    let taxed = pass[0]
        .assessed
        .iter()
        .find(|a| a.product == purchase.sku && a.venue == purchase.merchant)
        .cloned()
        .expect("the purchased candidate was assessed");
    // The same candidate with no border, assessed on the same synthetic feed.
    let free = simulated(&run("basis-free", commerce_plan()?, 1)?)?[0]
        .assessed
        .iter()
        .find(|a| a.product == taxed.product && a.venue == taxed.venue)
        .cloned()
        .expect("the duty-free run assessed the same candidate");

    // Premise: units were received, and only the taxed run paid a border.
    assert_eq!(Decimal::from_int(purchase.units.len() as i64), quantity);
    assert_eq!(
        (free.duty, free.purchase_tax),
        (Decimal::ZERO, Decimal::ZERO)
    );
    assert!(taxed.duty > Decimal::ZERO && taxed.purchase_tax > Decimal::ZERO);

    let ledger = platform.commerce_research().expect("installed").ledger();
    for unit in &purchase.units {
        let basis = ledger.units()[unit].cost_basis;
        assert_eq!(basis * quantity, taxed.all_in_cost, "unit {unit}");
        assert_eq!(
            basis * quantity - free.all_in_cost,
            taxed.duty + taxed.purchase_tax,
            "unit {unit}'s basis carries its share of the duty and the tax"
        );
    }
    Ok(())
}

/// Every sale the cycle makes is settled on the simulated marketplace at
/// gross less its fee, and the cycle's realised resale is that net (1094).
///
/// Mutation: price the fee at zero in `CommercePlane::settle` and the fee
/// assertion fails on a zero fee.
#[test]
fn every_paper_sale_is_settled_at_gross_less_the_marketplace_fee() -> Result<()> {
    let platform = run("settle", commerce_plan()?, 12)?;
    let research = platform.commerce_research().expect("installed");
    let sold: Vec<(&String, &String)> = research
        .ledger()
        .units()
        .iter()
        .filter_map(|(unit, record)| match &record.state {
            UnitState::Sold { sale } => Some((unit, sale)),
            _ => None,
        })
        .collect();
    assert!(!sold.is_empty(), "nothing was sold to settle");
    let fee_rate = ad_valorem();
    for (unit, sale) in sold {
        let settlement = research
            .plane()
            .settlement(sale)
            .unwrap_or_else(|| panic!("{unit}'s sale {sale} was never settled"));
        assert!(settlement.gross.is_positive());
        assert_eq!(settlement.fee, settlement.gross * fee_rate);
        assert_eq!(settlement.net, settlement.gross - settlement.fee);
        assert!(settlement.adjustments.is_empty());
    }
    for cycle in research.cycles() {
        assert!(!cycle.returned);
        assert!(cycle.resale_price.is_positive());
    }
    Ok(())
}

/// A plan with a return rate takes buyer returns at that rate (1092): with
/// half of sales returned, `floor(sales / 2)` sales come back, each unit is
/// back in the warehouse open-box and held, the ledger records the return
/// against its sale, and the settlement is adjusted by the refunded gross
/// less the ad valorem fee the marketplace gives back.
///
/// Mutation: make `take_return_owed` return `Ok(false)` without returning
/// and the ledger records no return.
#[test]
fn a_plan_with_a_return_rate_takes_buyer_returns_into_the_ledger_and_adjusts_their_settlement()
-> Result<()> {
    let mut plan = commerce_plan()?;
    plan.terms.returns.rate = dec!("0.5");
    let platform = run("returns", plan, 12)?;
    let research = platform.commerce_research().expect("installed");
    let cycles = research.cycles();
    // Premise: enough sales for the rate to owe a return.
    assert!(cycles.len() >= 2, "{} sales", cycles.len());
    let returns = research.ledger().returns();
    let returned_cycles = cycles.iter().filter(|c| c.returned).count();
    assert_eq!(returns.len(), cycles.len() / 2);
    assert_eq!(returned_cycles, returns.len());
    let fee_rate = ad_valorem();
    for record in returns {
        assert_eq!(record.condition, Condition::OpenBox);
        let unit = &research.ledger().units()[&record.unit];
        assert_eq!(unit.state, UnitState::Held);
        assert_eq!(unit.location, WAREHOUSE);
        assert_eq!(unit.owner, OWNER, "a returned unit is the desk's again");
        let settlement = research.plane().settlement(&record.sale).expect("settled");
        assert_eq!(
            settlement.net_after_adjustments(),
            settlement.net - settlement.gross + settlement.gross * fee_rate
        );
        assert!(research.ledger().events().iter().any(|event| matches!(
            event,
            LedgerEvent::Returned { sale, .. } if sale == &record.sale
        )));
    }
    Ok(())
}

// ------------------------------------------------- the committed dev plan

/// The plan dev's deep brain is to mount (ADR 0245; the mount itself is a
/// protected patch). These tests install the committed file through the
/// deep brain's own `breadth::install`, so they prove the file and the
/// binary's path, not a deployment.
fn committed_plan_path() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../data/breadth/dev-commerce-paper.json"
    )
    .to_string()
}

fn install_plan_file(path: &str, cycles: usize) -> Result<Platform> {
    let environment = BTreeMap::from([(PLAN_VARIABLE.to_string(), path.to_string())]);
    let mut platform = platform()?;
    install(
        &mut platform,
        &|name| environment.get(name).cloned(),
        start(),
    )?
    .expect("a plan was set");
    let mut at = start();
    for _ in 0..cycles {
        platform.run_cycle(at);
        at = at.saturating_add(Duration::from_hours(1));
    }
    Ok(platform)
}

/// R12-COMMERCE-5091. The committed dev plan, installed the way `main`
/// installs it, puts a commerce plane in the platform whose Product Scout
/// emits candidates on the first cycles.
///
/// Mutation: drop the commerce section from the committed plan and the
/// install refuses a plan with no section, so the test fails.
#[test]
fn the_committed_dev_plan_installs_a_commerce_plane_whose_scout_emits_candidates() -> Result<()> {
    let platform = install_plan_file(&committed_plan_path(), 3)?;
    assert!(
        platform.commerce_research().is_some(),
        "the committed plan installed no commerce plane"
    );
    let found = discovered(&platform)?;
    assert!(!found.is_empty(), "premise: DISCOVER journaled nothing");
    assert!(
        found.iter().any(|run| !run.candidates.is_empty()),
        "the scout emitted no candidate under the committed plan: {found:?}"
    );
    Ok(())
}

/// R12-COMMERCE-5092. Under the committed plan's own counterfeit registry,
/// the synthetic feed's listings of one product at different merchants
/// resolve to one identity, and both kinds of counterfeit are refused: the
/// GTIN on the registry and the seller on it. The resolver is the plane's,
/// built from the plan the deep brain loads; the cycle half shows the
/// refusals reach DISCOVER's journal rather than only the library.
///
/// Mutation: empty `counterfeit_gtins` in the committed plan and the test
/// fails on the GTIN refusal.
#[test]
fn the_committed_dev_plan_maps_its_equivalent_listings_to_one_identity_and_refuses_both_counterfeits()
-> Result<()> {
    use qip_financial::commerce::Resolver;
    use qip_kernel::commerce_research::CommerceFeed;
    let path = committed_plan_path();
    let environment = BTreeMap::from([(PLAN_VARIABLE.to_string(), path.clone())]);
    let plan = qip_deepbrain::breadth::load(&|name| environment.get(name).cloned())?
        .expect("the committed plan loads")
        .commerce
        .expect("the committed plan carries a commerce section");
    let gtin = plan
        .counterfeit_gtins
        .iter()
        .next()
        .cloned()
        .expect("the committed plan names a counterfeit GTIN");
    let resolver = Resolver::new(
        plan.counterfeit_gtins.clone(),
        plan.counterfeit_sellers.clone(),
    );

    let mut feed = SyntheticCommerceFeed::new(7, start());
    let mut at = start();
    let mut gtin_refused = false;
    let mut seller_refused = false;
    let mut merged = false;
    for _ in 0..24 {
        let batch = feed.poll(at)?;
        at = at.saturating_add(Duration::from_hours(1));
        let identities = resolver.resolve(&batch.listings)?;
        let mut by_identity: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for listing in &batch.listings {
            let identity = identities.get(&listing.id).expect("every listing resolves");
            match &identity.counterfeit_finding {
                Some(finding) => {
                    if listing.gtin.as_deref() == Some(gtin.as_str()) {
                        assert!(finding.contains(&gtin), "{finding}");
                        gtin_refused = true;
                    } else if listing.seller == SYNTHETIC_COUNTERFEIT_SELLER {
                        assert!(finding.contains(SYNTHETIC_COUNTERFEIT_SELLER), "{finding}");
                        seller_refused = true;
                    }
                }
                None => {
                    assert_ne!(
                        listing.gtin.as_deref(),
                        Some(gtin.as_str()),
                        "a listing under the registry's GTIN resolved as genuine"
                    );
                    by_identity
                        .entry(identity.id.clone())
                        .or_default()
                        .insert(listing.seller.clone());
                }
            }
        }
        if by_identity.values().any(|sellers| sellers.len() >= 2) {
            merged = true;
        }
    }
    assert!(
        merged,
        "no product's listings at two different merchants resolved to one identity"
    );
    assert!(
        gtin_refused,
        "no listing under the counterfeit GTIN {gtin} was refused"
    );
    assert!(
        seller_refused,
        "no listing from the counterfeit seller was refused"
    );

    // Through the cycle: DISCOVER journals the GTIN refusal by name.
    let platform = install_plan_file(&path, 12)?;
    let refusals: Vec<String> = discovered(&platform)?
        .into_iter()
        .flat_map(|run| run.counterfeit)
        .collect();
    assert!(
        refusals
            .iter()
            .any(|refusal| refusal.contains(&format!("GTIN {gtin}"))),
        "DISCOVER journaled no counterfeit-GTIN refusal under the committed plan: {refusals:?}"
    );
    Ok(())
}

/// R12-COMMERCE-5093. A cycle under the committed plan journals, for every
/// candidate it assesses, the six figures feasibility owes: all-in cost,
/// expected resale, inventory carry, capital lockup, time to sale and return
/// risk. A copy of the plan with one logistics input removed is refused: the
/// plane never assesses without it. The refusal lands at load, because the
/// plan's fields are required; `need()` inside `FeasibilityInput::assess` is
/// a second guard that a plan cannot reach, since the plane builds every
/// input from the plan as `Some`. The test accepts a refusal from either.
///
/// Mutation: give `LogisticsTerms::storage_days` a serde default and the
/// incomplete copy loads, so the refusal case fails.
#[test]
fn a_cycle_under_the_committed_dev_plan_journals_a_commerce_simulated_assessment_with_all_six_figures()
-> Result<()> {
    let platform = install_plan_file(&committed_plan_path(), 6)?;
    let assessed: Vec<_> = simulated(&platform)?
        .into_iter()
        .flat_map(|run| run.assessed)
        .collect();
    assert!(!assessed.is_empty(), "premise: nothing was assessed");
    for figure in &assessed {
        assert!(
            figure.all_in_cost.is_positive()
                && figure.expected_resale.is_positive()
                && figure.capital_lockup.is_positive()
                && figure.time_to_sale_days > 0
                && !figure.inventory_carry.is_negative()
                && !figure.return_risk.is_negative(),
            "an assessment is missing one of the six figures: {figure:?}"
        );
    }

    // One logistics input removed from a copy of the committed plan.
    let text = std::fs::read_to_string(committed_plan_path()).expect("the committed plan reads");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("the plan is JSON");
    let removed = value["commerce"]["terms"]
        .as_object_mut()
        .expect("the plan states logistics terms")
        .remove("storage_days");
    assert!(
        removed.is_some(),
        "premise: the committed plan states storage_days"
    );
    let path = std::env::temp_dir().join(format!(
        "qip-deepbrain-commerce-{}-no-storage-days.json",
        std::process::id()
    ));
    std::fs::write(&path, value.to_string()).expect("the copy is writable");
    let refusal = match install_plan_file(&path.to_string_lossy(), 6) {
        Ok(platform) => {
            let assessed: usize = simulated(&platform)?
                .iter()
                .map(|run| run.assessed.len())
                .sum();
            panic!(
                "a plan with no storage_days was installed and assessed {assessed} candidate(s) \
                 rather than refused"
            )
        }
        Err(refusal) => refusal,
    };
    assert!(
        refusal.message().contains("storage_days")
            || refusal.message().contains("feasibility needs"),
        "refused for a reason other than the missing input: {}",
        refusal.message()
    );
    Ok(())
}
