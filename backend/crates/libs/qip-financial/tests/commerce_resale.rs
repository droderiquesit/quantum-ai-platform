//! Contract tests for unit attributes, the resale router, resale pricing,
//! SKU learning and the end-to-end simulated lifecycle.

// Fixture helpers state their premises with `expect`.
#![allow(clippy::expect_used)]

use qip_core::{Decimal, dec};
use qip_financial::commerce::*;
use qip_financial::commerce_resale::*;
use qip_financial::physical::{
    Customs, Leg, LogisticsTerms, MarketplaceFees, Returns, Spoilage, TransportMode,
};
use std::collections::{BTreeMap, BTreeSet};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn fees() -> MarketplaceFees {
    MarketplaceFees {
        ad_valorem: dec!("0.1"),
        per_unit: dec!("0.5"),
        per_consignment: dec!("2"),
    }
}

fn terms() -> LogisticsTerms {
    LogisticsTerms {
        route: vec![Leg {
            from: "Shenzhen".into(),
            to: "Rotterdam".into(),
            mode: TransportMode::Sea,
            days: 10,
            freight_per_unit: dec!("2"),
            freight_per_shipment: Decimal::ZERO,
        }],
        customs: Customs {
            duty_rate: dec!("0.1"),
            clearance_fee: dec!("5"),
            clearance_days: 0,
        },
        spoilage: Spoilage::none("a non-perishable good, stated by the desk").expect("stated"),
        storage_per_unit_per_day: Decimal::ZERO,
        storage_days: 0,
        fees: fees(),
        returns: Returns {
            rate: Decimal::ZERO,
            cost_per_unit: Decimal::ZERO,
            recovery_rate: Decimal::ZERO,
        },
    }
}

fn input(days: u32) -> FeasibilityInput {
    FeasibilityInput {
        unit_price: Some(dec!("20")),
        quantity: Some(dec!("10")),
        declared_value_per_unit: Some(dec!("20")),
        terms: Some(terms()),
        resale: vec![(dec!("40"), dec!("0.5")), (dec!("30"), dec!("0.5"))],
        days_on_market: Some(days),
        daily_capital_cost: Some(dec!("0.001")),
        purchase_tax_rate: Some(dec!("0.05")),
    }
}

fn received(unit: &str, basis: Decimal) -> LedgerEvent {
    LedgerEvent::Received {
        unit: unit.into(),
        sku: "sku-1".into(),
        condition: Condition::New,
        location: "Rotterdam-DC1".into(),
        cost_basis: basis,
        owner: "platform".into(),
    }
}

fn plane() -> CommercePlane {
    let mut l = Ledger::default();
    l.apply(received("u1", dec!("29.7"))).expect("received");
    CommercePlane::new(l, fees())
}

// ---- Unit attributes (COMMERCE-007)

fn full_record(r: &mut Lcg) -> BTreeMap<String, Attr> {
    UNIT_ATTRIBUTES
        .iter()
        .map(|n| {
            let a = if r.next(2) == 0 {
                Attr::Value(format!("{n}-{}", r.next(1000)))
            } else {
                Attr::Absent {
                    reason: format!("{n} does not apply, decided {}", r.next(1000)),
                }
            };
            (n.to_string(), a)
        })
        .collect()
}

#[test]
fn a_unit_record_missing_any_of_the_twelve_attributes_is_refused_naming_it_and_a_full_one_is_kept()
{
    let mut r = Lcg(7);
    assert_eq!(UNIT_ATTRIBUTES.len(), 12, "premise: twelve attributes");
    for round in 0..40 {
        let full = full_record(&mut r);
        let kept = UnitAttributes::new("u1", full.clone()).expect("a full record is built");
        for n in UNIT_ATTRIBUTES {
            assert!(
                kept.get(n).is_some(),
                "round {round}: {n} absent from a built record"
            );
        }
        for n in UNIT_ATTRIBUTES {
            let mut short = full.clone();
            short.remove(n);
            let e = UnitAttributes::new("u1", short).expect_err("a short record was built");
            assert!(e.to_string().contains(&format!("lacks {n};")), "{e}");
        }
    }
    let mut blank = full_record(&mut r);
    blank.insert("tax".into(), Attr::Absent { reason: " ".into() });
    assert!(
        UnitAttributes::new("u1", blank).is_err(),
        "a reasonless absence was kept"
    );
    let mut extra = full_record(&mut r);
    extra.insert("colour".into(), Attr::Value("red".into()));
    assert!(
        UnitAttributes::new("u1", extra).is_err(),
        "an unknown attribute was kept"
    );
}

// ---- Resale router (COMMERCE-019)

fn quote(kind: VenueKind, price: Decimal, fee: Decimal, fixed: Decimal, days: u32) -> VenueQuote {
    VenueQuote {
        kind,
        venue: format!("{kind:?}-1"),
        expected_price: price,
        fee_rate: fee,
        fixed_fee: fixed,
        days_to_sale: days,
    }
}

fn quotes() -> Vec<VenueQuote> {
    vec![
        quote(
            VenueKind::Marketplace,
            dec!("150"),
            dec!("0.1"),
            dec!("2"),
            10,
        ),
        quote(
            VenueKind::Auction,
            dec!("170"),
            dec!("0.2"),
            Decimal::ZERO,
            20,
        ),
        quote(
            VenueKind::Direct,
            dec!("140"),
            Decimal::ZERO,
            Decimal::ZERO,
            3,
        ),
    ]
}

#[test]
fn the_router_picks_the_best_net_venue_not_the_best_price_records_its_figures_and_repeats_on_replay()
 {
    // Marketplace 150 - 15 - 2 = 133, carry 1 => 132. Auction 170 - 34 = 136,
    // carry 2 => 134. Direct 140, carry 0.3 => 139.7. The highest gross price
    // is the auction's; the best net is direct.
    let q = quotes();
    assert!(
        q.iter().max_by_key(|x| x.expected_price).map(|x| x.kind) == Some(VenueKind::Auction),
        "premise: the highest price is not the winner"
    );
    let c = route_resale(dec!("100"), dec!("0.001"), &q).expect("routed");
    assert_eq!(c.chosen.kind, VenueKind::Direct);
    assert_eq!(c.chosen.score, dec!("139.7"));
    let scores: Vec<_> = c.rationale.iter().map(|f| f.score).collect();
    assert_eq!(scores, vec![dec!("132"), dec!("134"), dec!("139.7")]);
    let mut reversed = q.clone();
    reversed.reverse();
    assert_eq!(
        route_resale(dec!("100"), dec!("0.001"), &reversed).expect("replayed"),
        c,
        "a different quote order changed the choice"
    );
    assert!(route_resale(dec!("100"), dec!("0.001"), &[]).is_err());
    let mut bad = q;
    bad[0].fee_rate = dec!("1");
    assert!(route_resale(dec!("100"), dec!("0.001"), &bad).is_err());
}

// ---- Resale pricing (COMMERCE-020)

#[test]
fn a_listed_unit_is_repriced_on_a_market_move_or_a_stale_interval_and_a_sold_unit_never_is() {
    let mut p = plane();
    let mut d = ResaleDesk::new(dec!("50"), 14, dec!("0.1")).expect("desk");
    assert_eq!(
        d.list(&p, "u1", dec!("100"), 0).expect("listed"),
        dec!("100")
    );
    assert_eq!(d.reprice(&p, "u1", dec!("100"), 5).expect("quiet"), None);
    assert_eq!(
        d.reprice(&p, "u1", dec!("90"), 5).expect("moved"),
        Some(dec!("90"))
    );
    assert_eq!(d.reprice(&p, "u1", dec!("90"), 10).expect("quiet"), None);
    assert_eq!(
        d.reprice(&p, "u1", dec!("90"), 20).expect("stale"),
        Some(dec!("81"))
    );
    let h = &d.listed("u1").expect("listed").history;
    assert_eq!(h.len(), 3, "every publication is recorded: {h:?}");

    p.sell("s1", "u1", "buyer", dec!("81"), None).expect("sold");
    let before = d.listed("u1").cloned();
    assert!(
        d.reprice(&p, "u1", dec!("70"), 40).is_err(),
        "a sold unit was repriced"
    );
    assert_eq!(
        d.listed("u1").cloned(),
        before,
        "a refused repricing changed the listing"
    );
}

#[test]
fn a_repricing_under_the_floor_is_refused_and_leaves_the_price_alone() {
    let p = plane();
    let mut d = ResaleDesk::new(dec!("85"), 14, dec!("0.1")).expect("desk");
    d.list(&p, "u1", dec!("90"), 0).expect("listed");
    assert!(
        d.reprice(&p, "u1", dec!("90"), 20).is_err(),
        "marked down through the floor"
    );
    assert_eq!(d.listed("u1").map(|l| l.price), Some(dec!("90")));
}

// ---- SKU learning (COMMERCE-005)

fn cycle(resale: Decimal, days: u32, returned: bool) -> CycleEvent {
    CycleEvent {
        sku: "sku-1".into(),
        realised_cost: dec!("30"),
        resale_price: resale,
        days_to_sale: days,
        returned,
    }
}

#[test]
fn replayed_cycles_reproduce_the_sku_record_and_the_next_assessment_moves_by_exactly_what_was_learned()
 {
    let log = vec![
        cycle(dec!("150"), 10, false),
        cycle(dec!("170"), 20, true),
        cycle(dec!("160"), 30, false),
        cycle(dec!("160"), 20, false),
    ];
    let a = replay_economics(&log).expect("replayed");
    assert_eq!(a, replay_economics(&log).expect("again"), "replay differed");
    let e = &a["sku-1"];
    assert_eq!(
        (e.cycles, e.total_resale, e.total_days, e.returns),
        (4, dec!("640"), 80, 1)
    );

    let before = input(40).assess().expect("before");
    let after = input(40)
        .with_learned(e)
        .expect("learned")
        .assess()
        .expect("after");
    // Mean resale 160, mean days 20, return rate 0.25. 7.5 of 10 units sell
    // (recovery 0): 7.5 x 160 = 1200 against 350 before.
    assert_eq!(before.expected_resale, dec!("350"));
    assert_eq!(after.expected_resale, dec!("1200"));
    assert_eq!(before.time_to_sale_days - after.time_to_sale_days, 20);
    assert_eq!(after.return_risk - before.return_risk, dec!("0.25"));
    let empty = SkuEconomics {
        cycles: 0,
        total_cost: Decimal::ZERO,
        total_resale: Decimal::ZERO,
        total_days: 0,
        returns: 0,
    };
    assert!(input(40).with_learned(&empty).is_err());
}

// ---- The whole lifecycle (COMMERCE-001)

#[test]
fn one_simulated_product_runs_from_discovery_to_a_settled_resale_with_every_stage_present() {
    let found = scout(&[Observation {
        product: "phone x".into(),
        venue: "auction-eu".into(),
        channel: Channel::Auction,
        price: dec!("20"),
        reference_price: dec!("40"),
        units_available: 10,
        units_previously_available: 10,
    }]);
    assert_eq!(found.len(), 1, "premise: scout found the opportunity");
    assert_eq!(found[0].kind, OpportunityKind::Auction);

    let ls = [Listing {
        id: "auction-eu-1".into(),
        brand: "Acme".into(),
        model: "Phone X".into(),
        variant: "256GB".into(),
        gtin: Some("0001".into()),
        seller: "acme".into(),
    }];
    let ids = Resolver::default().resolve(&ls).expect("resolved");
    assert!(ids["auction-eu-1"].counterfeit_finding.is_none());

    let a = input(20).assess().expect("assessed");
    assert_eq!(a.all_in_cost, dec!("297"));
    assert!(a.expected_resale > a.all_in_cost, "premise: worth buying");

    let mut ex = PurchaseExecutor::new(
        vec![Account {
            id: "acct-1".into(),
            identity_class: "commerce-buyer".into(),
            merchants: BTreeSet::from(["shop-a".to_string()]),
            per_purchase_limit: dec!("500"),
        }],
        FraudBook::default(),
        dec!("800"),
    );
    let rec = ex
        .purchase(&PurchaseRequest {
            account_id: "acct-1".into(),
            credential: Credential {
                account_id: "acct-1".into(),
                identity_class: "commerce-buyer".into(),
            },
            merchant: "shop-a".into(),
            listing: "auction-eu-1".into(),
            payment: "card-ok".into(),
            sku: "sku-1".into(),
            quantity: dec!("10"),
            unit_price: dec!("20"),
        })
        .expect("purchased");
    assert_eq!(rec.notional, dec!("200"));
    assert_eq!(ex.merchant_calls(), 1);

    // Ten units cost 297 all-in; the ledger holds one of them at 29.7.
    let basis = dec!("29.7");
    let mut p = plane();
    assert_eq!(p.ledger().units()["u1"].cost_basis, basis);

    let q = quotes();
    let c = route_resale(basis, dec!("0.001"), &q).expect("routed");
    let price = q
        .iter()
        .find(|x| x.venue == c.chosen.venue)
        .map(|x| x.expected_price)
        .expect("the chosen venue was quoted");
    let mut d = ResaleDesk::new(basis, 14, dec!("0.1")).expect("desk");
    assert_eq!(d.list(&p, "u1", price, 0).expect("listed"), price);

    p.sell("s1", "u1", "buyer", price, None).expect("sold");
    assert!(!p.is_settled("s1"));
    let s = p.settle("s1").expect("settled");
    assert!(s.net > Decimal::ZERO);
}

// ---- Customs duty (COMMERCE-003)

#[test]
fn cross_border_purchase_computes_customs_duty_as_fixture_expects() {
    let declared_value_per_unit = dec!("50");
    let quantity = dec!("100");
    let unit_purchase_price = dec!("50");
    let customs_duty_rate = dec!("0.08");
    let freight_per_unit = dec!("3");
    let clearance_fee = dec!("25");

    let terms = LogisticsTerms {
        route: vec![Leg {
            from: "Shanghai".into(),
            to: "Newark".into(),
            mode: TransportMode::Sea,
            days: 21,
            freight_per_unit,
            freight_per_shipment: Decimal::ZERO,
        }],
        customs: Customs {
            duty_rate: customs_duty_rate,
            clearance_fee,
            clearance_days: 2,
        },
        spoilage: Spoilage::none("electronics do not perish in transit").expect("stated"),
        storage_per_unit_per_day: dec!("0.01"),
        storage_days: 2,
        fees: MarketplaceFees {
            ad_valorem: Decimal::ZERO,
            per_unit: Decimal::ZERO,
            per_consignment: Decimal::ZERO,
        },
        returns: Returns {
            rate: Decimal::ZERO,
            cost_per_unit: Decimal::ZERO,
            recovery_rate: Decimal::ZERO,
        },
    };

    let cost = terms
        .cost(quantity, unit_purchase_price, declared_value_per_unit)
        .expect("landed cost computed");

    let total_declared_value = declared_value_per_unit * quantity;
    let expected_customs_duty = total_declared_value * customs_duty_rate;
    let expected_total_freight = freight_per_unit * quantity;
    let elapsed_days = dec!("25"); // clearance_days (2) + sea transit (21) + storage_days (2)
    let expected_storage = quantity * dec!("0.01") * elapsed_days;

    assert_eq!(
        cost.duty, expected_customs_duty,
        "customs duty must equal declared value × duty rate: {} = {} × {}",
        expected_customs_duty, total_declared_value, customs_duty_rate
    );
    assert_eq!(
        cost.clearance, clearance_fee,
        "clearance fee must match fixture"
    );
    assert_eq!(
        cost.freight, expected_total_freight,
        "total freight must equal freight_per_unit × quantity"
    );
    assert_eq!(
        cost.storage, expected_storage,
        "storage must equal quantity × storage_rate × elapsed_days"
    );

    let expected_total_cost = unit_purchase_price * quantity
        + expected_total_freight
        + expected_customs_duty
        + clearance_fee
        + expected_storage;

    assert_eq!(
        cost.total(),
        expected_total_cost,
        "total landed cost must equal sum of all components"
    );

    let cost_per_delivered_unit = cost.per_delivered_unit().expect("delivered unit cost");
    assert!(
        cost_per_delivered_unit > unit_purchase_price,
        "cost per delivered unit must exceed purchase price"
    );
}
