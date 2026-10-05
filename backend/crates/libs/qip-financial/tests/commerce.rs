//! Contract tests for the simulated commerce plane.

use qip_core::{Decimal, dec};
use qip_financial::commerce::*;
use qip_financial::physical::{
    Customs, Leg, LogisticsTerms, MarketplaceFees, Returns, Spoilage, TransportMode,
};
use std::collections::BTreeSet;

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Deterministic generator so the property tests need no dependency.
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

// ---- Scout (COMMERCE-008)

fn obs(channel: Channel, price: Decimal, avail: u32, prev: u32) -> Observation {
    Observation {
        product: "widget".into(),
        venue: format!("{channel:?}"),
        channel,
        price,
        reference_price: dec!("100"),
        units_available: avail,
        units_previously_available: prev,
    }
}

#[test]
fn product_scout_emits_one_typed_candidate_per_opportunity_kind_and_none_for_a_quiet_market() {
    let quiet = [
        obs(Channel::Retail, dec!("100"), 10, 10),
        obs(Channel::Auction, dec!("100"), 1, 1),
        obs(Channel::Wholesale, dec!("120"), 50, 50),
    ];
    assert!(!quiet.is_empty(), "premise: the quiet fixture has readings");
    assert!(scout(&quiet).is_empty(), "a quiet market produced a find");

    let busy = [
        obs(Channel::Retail, dec!("80"), 10, 10),
        obs(Channel::Auction, dec!("60"), 1, 1),
        obs(Channel::Liquidation, dec!("40"), 500, 500),
        obs(Channel::Wholesale, dec!("55"), 1000, 1000),
        obs(Channel::Retail, dec!("100"), 2, 10),
    ];
    let kinds: Vec<OpportunityKind> = scout(&busy).iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        vec![
            OpportunityKind::PriceGap,
            OpportunityKind::Auction,
            OpportunityKind::Liquidation,
            OpportunityKind::Wholesale,
            OpportunityKind::SupplyShock
        ]
    );
}

// ---- Resolver (COMMERCE-009, -010)

fn listing(
    id: &str,
    brand: &str,
    model: &str,
    variant: &str,
    gtin: Option<&str>,
    seller: &str,
) -> Listing {
    Listing {
        id: id.into(),
        brand: brand.into(),
        model: model.into(),
        variant: variant.into(),
        gtin: gtin.map(String::from),
        seller: seller.into(),
    }
}

#[test]
fn listings_of_one_product_under_different_identifiers_resolve_to_one_identity() {
    let ls = [
        listing(
            "amazon-us-1",
            "Acme",
            "Phone X",
            "256GB Black",
            Some("0001"),
            "acme",
        ),
        listing("ebay-uk-9", "ACME", "phone  x", "256gb black", None, "shop"),
        listing(
            "de-retail-3",
            "Acme",
            "Phone X",
            "256GB Black",
            Some("0001"),
            "mart",
        ),
    ];
    let r = Resolver::default().resolve(&ls).expect("resolves");
    let ids: BTreeSet<_> = r.values().map(|i| i.id.clone()).collect();
    assert_eq!(r.len(), 3, "premise: every listing resolved");
    assert_eq!(ids.len(), 1, "one product resolved to {ids:?}");
}

#[test]
fn a_gtin_shared_by_listings_of_different_model_text_is_refused_rather_than_merged() {
    // A matches B on text, B matches C on GTIN: A and C are one product.
    let ls = [
        listing("a", "Acme", "X", "1", None, "s"),
        listing("b", "Acme", "X", "1", Some("77"), "s"),
        listing("c", "Acme", "X Pro rebadged", "1", Some("77"), "s"),
    ];
    let err = Resolver::default().resolve(&ls);
    // Different model text but one GTIN is a feed conflict: refused.
    assert!(err.is_err(), "a GTIN merged two different model texts");
}

#[test]
fn a_genuine_listing_a_variant_and_a_counterfeit_resolve_to_three_distinct_identities() {
    let ls = [
        listing("g", "Acme", "Phone X", "256GB", Some("100"), "acme"),
        listing("v", "Acme", "Phone X", "512GB", Some("101"), "acme"),
        listing("f", "Acme", "Phone X", "256GB", Some("666"), "bargain"),
    ];
    let r = Resolver::new(set(&["666"]), BTreeSet::new())
        .resolve(&ls)
        .expect("resolves");
    let ids: BTreeSet<_> = r.values().map(|i| i.id.clone()).collect();
    assert_eq!(ids.len(), 3, "got {ids:?}");
    assert!(r["g"].counterfeit_finding.is_none());
    assert!(r["v"].counterfeit_finding.is_none());
    assert!(
        r["f"]
            .counterfeit_finding
            .as_deref()
            .is_some_and(|f| f.contains("666")),
        "the counterfeit carries an explicit finding"
    );
    assert_ne!(r["f"].id, r["g"].id);
}

#[test]
fn resolution_over_generated_listing_sets_is_the_same_partition_in_any_order() {
    let mut rng = Lcg(7);
    for _ in 0..200 {
        let n = 3 + rng.next(10) as usize;
        let mut ls = Vec::new();
        let mut truth = Vec::new();
        for i in 0..n {
            let (p, v) = (rng.next(3), rng.next(2));
            let src = rng.next(4);
            truth.push((p, v));
            // GTIN is a pure function of (product, variant), like a real one.
            let gtin = (rng.next(2) == 0).then(|| format!("g{p}{v}"));
            ls.push(listing(
                &format!("l{i}"),
                "Brand",
                &format!("Model {p}"),
                &format!("v{v}"),
                gtin.as_deref(),
                &format!("seller{src}"),
            ));
        }
        let forward = Resolver::default().resolve(&ls).expect("resolves");
        ls.reverse();
        let backward = Resolver::default().resolve(&ls).expect("resolves");
        assert_eq!(forward, backward, "order changed the resolution");
        for i in 0..n {
            for j in 0..n {
                let same = forward[&format!("l{i}")] == forward[&format!("l{j}")];
                assert_eq!(same, truth[i] == truth[j], "pair {i},{j}");
                assert_eq!(same, forward[&format!("l{j}")] == forward[&format!("l{i}")]);
            }
        }
    }
}

// ---- Feasibility (COMMERCE-011)

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
        fees: MarketplaceFees {
            ad_valorem: dec!("0.1"),
            per_unit: dec!("0.5"),
            per_consignment: dec!("2"),
        },
        returns: Returns {
            rate: Decimal::ZERO,
            cost_per_unit: Decimal::ZERO,
            recovery_rate: Decimal::ZERO,
        },
    }
}

fn input() -> FeasibilityInput {
    FeasibilityInput {
        unit_price: Some(dec!("20")),
        quantity: Some(dec!("10")),
        declared_value_per_unit: Some(dec!("20")),
        terms: Some(terms()),
        resale: vec![(dec!("40"), dec!("0.5")), (dec!("30"), dec!("0.5"))],
        days_on_market: Some(20),
        daily_capital_cost: Some(dec!("0.001")),
        purchase_tax_rate: Some(dec!("0.05")),
    }
}

#[test]
fn feasibility_returns_all_six_figures_equal_to_the_hand_computation() {
    // goods 200, freight 20, duty 20, clearance 5, tax 10 (5% of goods);
    // resale 10 x 35 = 350; marketplace 10% of 350 + 0.5 x 10 + 2 = 42;
    // all-in 200+20+20+5+42+10 = 297. Time 10 + 20 days. Lockup
    // 200+20+20+5+10 = 255; carry 255 x 0.001 x 30 = 7.65.
    let a = input().assess().expect("assessed");
    assert_eq!(a.all_in_cost, dec!("297"));
    assert_eq!(a.expected_resale, dec!("350"));
    assert_eq!(a.time_to_sale_days, 30);
    assert_eq!(a.return_risk, Decimal::ZERO);
    assert_eq!(a.inventory_carry, dec!("7.65"));
    assert_eq!(a.capital_lockup, dec!("255"));
}

#[test]
fn a_candidate_lacking_any_input_a_figure_needs_is_refused_naming_it() {
    let mut cases: Vec<(&str, FeasibilityInput)> = Vec::new();
    let mut i = input();
    i.unit_price = None;
    cases.push(("unit price", i));
    let mut i = input();
    i.terms = None;
    cases.push(("logistics terms", i));
    let mut i = input();
    i.days_on_market = None;
    cases.push(("days on market", i));
    let mut i = input();
    i.daily_capital_cost = None;
    cases.push(("cost of capital", i));
    let mut i = input();
    i.purchase_tax_rate = None;
    cases.push(("purchase tax", i));
    let mut i = input();
    i.resale.clear();
    cases.push(("resale price distribution", i));
    assert!(input().assess().is_ok(), "premise: the full input assesses");
    for (what, i) in cases {
        let e = i.assess().expect_err("an incomplete input was assessed");
        assert!(e.message().contains(what), "{what}: {}", e.message());
    }
}

// ---- Purchase controls (COMMERCE-013, -014, -015)

fn executor() -> PurchaseExecutor {
    PurchaseExecutor::new(
        vec![Account {
            id: "acct-1".into(),
            identity_class: "commerce-buyer".into(),
            merchants: set(&["shop-a"]),
            per_purchase_limit: dec!("500"),
        }],
        FraudBook {
            merchants: set(&["shop-scam"]),
            listings: set(&["listing-bad"]),
            payments: set(&["card-stolen"]),
        },
        dec!("800"),
    )
}

fn request(price: Decimal) -> PurchaseRequest {
    PurchaseRequest {
        account_id: "acct-1".into(),
        credential: Credential {
            account_id: "acct-1".into(),
            identity_class: "commerce-buyer".into(),
        },
        merchant: "shop-a".into(),
        listing: "listing-ok".into(),
        payment: "card-ok".into(),
        sku: "sku-1".into(),
        quantity: dec!("1"),
        unit_price: price,
    }
}

#[test]
fn a_purchase_over_the_limit_is_refused_with_no_record_and_no_merchant_call_and_one_under_is_admitted()
 {
    let mut e = executor();
    let refused = e.purchase(&request(dec!("501")));
    assert!(refused.is_err());
    assert_eq!(e.merchant_calls(), 0, "the merchant was called");
    assert!(e.records().is_empty(), "a refused purchase left a record");
    let ok = e.purchase(&request(dec!("500"))).expect("under the limit");
    assert_eq!(ok.notional, dec!("500"));
    assert_eq!((e.merchant_calls(), e.records().len()), (1, 1));
    // The running ceiling is a limit too: 500 + 500 > 800.
    assert!(e.purchase(&request(dec!("500"))).is_err());
    assert_eq!(e.merchant_calls(), 1);
}

#[test]
fn a_purchase_without_authority_or_with_another_identitys_credential_is_refused_before_the_merchant()
 {
    let mut e = executor();
    let mut wrong_class = request(dec!("10"));
    wrong_class.credential.identity_class = "research-agent".into();
    let mut other_account = request(dec!("10"));
    other_account.credential.account_id = "acct-2".into();
    let mut unlisted = request(dec!("10"));
    unlisted.merchant = "shop-b".into();
    let mut unknown = request(dec!("10"));
    unknown.account_id = "acct-9".into();
    for r in [wrong_class, other_account, unlisted, unknown] {
        assert!(e.purchase(&r).is_err());
    }
    assert_eq!(e.merchant_calls(), 0);
    assert!(e.purchase(&request(dec!("10"))).is_ok());
    assert_eq!(e.merchant_calls(), 1);
}

#[test]
fn a_fraud_finding_on_merchant_listing_or_payment_refuses_before_any_payment() {
    let mut e = executor();
    let mut m = request(dec!("10"));
    m.merchant = "shop-scam".into();
    let mut l = request(dec!("10"));
    l.listing = "listing-bad".into();
    let mut p = request(dec!("10"));
    p.payment = "card-stolen".into();
    let mut e2 = PurchaseExecutor::new(
        vec![Account {
            id: "acct-1".into(),
            identity_class: "commerce-buyer".into(),
            merchants: set(&["shop-a", "shop-scam"]),
            per_purchase_limit: dec!("500"),
        }],
        FraudBook {
            merchants: set(&["shop-scam"]),
            listings: set(&["listing-bad"]),
            payments: set(&["card-stolen"]),
        },
        dec!("800"),
    );
    for r in [m, l, p] {
        let err = e2.purchase(&r).expect_err("fraud was admitted");
        assert!(err.message().contains("fraud finding"), "{}", err.message());
    }
    assert_eq!(e2.merchant_calls(), 0);
    assert!(
        e.purchase(&request(dec!("10"))).is_ok(),
        "the clean twin is admitted"
    );
}

// ---- Logistics plan (COMMERCE-016)

fn draft() -> LogisticsPlanDraft {
    LogisticsPlanDraft {
        route: terms().route,
        carrier: Some("Maersk".into()),
        consolidation: Some(Consolidation::Consolidate { with_shipments: 3 }),
        warehouse: Some("Rotterdam-DC1".into()),
        customs: Some(terms().customs),
        insurance: Some(Insurance::Declined {
            reason: "goods value under the premium threshold".into(),
        }),
        delivery_risk: Some(dec!("0.02")),
    }
}

#[test]
fn a_logistics_plan_missing_any_of_the_seven_parts_is_refused() {
    let plan = draft().into_plan().expect("the full draft is a plan");
    assert_eq!(plan.carrier, "Maersk");
    let mut cases: Vec<(&str, LogisticsPlanDraft)> = Vec::new();
    macro_rules! without {
        ($name:expr, $f:ident, $v:expr) => {{
            let mut d = draft();
            d.$f = $v;
            cases.push(($name, d));
        }};
    }
    without!("route", route, Vec::new());
    without!("carrier", carrier, None);
    without!("consolidation", consolidation, None);
    without!("warehouse", warehouse, None);
    without!("customs", customs, None);
    without!("insurance", insurance, None);
    without!("delivery-risk", delivery_risk, None);
    assert_eq!(cases.len(), 7);
    for (name, d) in cases {
        let e = d.into_plan().expect_err("a partial plan was accepted");
        assert!(e.message().contains(name), "{name}: {}", e.message());
    }
    let mut bad = draft();
    bad.delivery_risk = Some(dec!("1.5"));
    assert!(bad.into_plan().is_err(), "a risk above one was clamped");
}

// ---- Ledger (COMMERCE-017, -018)

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

#[test]
fn replaying_the_event_log_reproduces_every_unit_exactly() {
    let mut l = Ledger::default();
    for e in [
        received("u1", dec!("29.7")),
        received("u2", dec!("31")),
        LedgerEvent::Moved {
            unit: "u1".into(),
            to: "Berlin-DC2".into(),
        },
        LedgerEvent::Reserved {
            unit: "u2".into(),
            reservation: "r1".into(),
        },
        LedgerEvent::Sold {
            unit: "u2".into(),
            reservation: Some("r1".into()),
            sale: "s1".into(),
            buyer: "buyer-9".into(),
        },
    ] {
        l.apply(e).expect("applies");
    }
    assert_eq!(l.units().len(), 2, "premise: two units");
    let replayed = Ledger::replay(l.events()).expect("replays");
    assert_eq!(replayed, l);
    let u1 = &replayed.units()["u1"];
    assert_eq!(
        (u1.location.as_str(), u1.cost_basis, u1.owner.as_str()),
        ("Berlin-DC2", dec!("29.7"), "platform")
    );
    assert_eq!(replayed.units()["u2"].owner, "buyer-9");
}

#[test]
fn a_reserved_unit_is_never_reserved_twice_or_sold_to_another_buyer_and_refusals_change_nothing() {
    let mut rng = Lcg(42);
    let mut l = Ledger::default();
    for u in ["u0", "u1", "u2"] {
        l.apply(received(u, dec!("10"))).expect("received");
    }
    let (mut accepted, mut refused) = (0, 0);
    for _ in 0..3000 {
        let unit = format!("u{}", rng.next(3));
        let res = format!("r{}", rng.next(3));
        let event = match rng.next(3) {
            0 => LedgerEvent::Reserved {
                unit,
                reservation: res,
            },
            1 => LedgerEvent::Released {
                unit,
                reservation: res,
            },
            _ => LedgerEvent::Sold {
                unit,
                reservation: (rng.next(2) == 0).then_some(res),
                sale: format!("s{}", rng.next(1_000_000)),
                buyer: "b".into(),
            },
        };
        let before = l.clone();
        match l.apply(event.clone()) {
            Ok(()) => {
                accepted += 1;
                // Only a sale from a matching reservation or from free stock.
                if let LedgerEvent::Sold {
                    unit, reservation, ..
                } = &event
                {
                    let was = &before.units()[unit].state;
                    match (was, reservation) {
                        (UnitState::Held, None) => {}
                        (UnitState::Reserved { reservation: h }, Some(r)) => assert_eq!(h, r),
                        _ => panic!("sold from {was:?} under {reservation:?}"),
                    }
                }
                if let LedgerEvent::Reserved { unit, .. } = &event {
                    assert_eq!(before.units()[unit].state, UnitState::Held);
                }
            }
            Err(_) => {
                refused += 1;
                assert_eq!(l, before, "a refused {event:?} changed the ledger");
            }
        }
    }
    assert!(
        accepted > 0 && refused > 0,
        "premise: both outcomes were exercised"
    );
}

// ---- Returns and settlement (COMMERCE-002, -004)

fn plane() -> CommercePlane {
    let mut l = Ledger::default();
    l.apply(received("u1", dec!("29.7"))).expect("received");
    CommercePlane::new(
        l,
        MarketplaceFees {
            ad_valorem: dec!("0.1"),
            per_unit: dec!("0.5"),
            per_consignment: dec!("2"),
        },
    )
}

#[test]
fn a_sale_is_not_settled_until_its_record_exists_and_nets_gross_less_the_fee_schedule() {
    let mut p = plane();
    p.sell("s1", "u1", "buyer", dec!("40"), None).expect("sold");
    assert!(
        !p.is_settled("s1"),
        "reported settled before a record existed"
    );
    let s = p.settle("s1").expect("settled").clone();
    assert_eq!(
        (s.gross, s.fee, s.net),
        (dec!("40"), dec!("6.5"), dec!("33.5"))
    );
    assert!(p.is_settled("s1"));
    assert!(p.settle("s1").is_err(), "settled twice");
}

#[test]
fn a_buyer_return_links_to_the_sale_restores_the_unit_and_adjusts_the_settlement() {
    let mut p = plane();
    p.sell("s1", "u1", "buyer", dec!("40"), None).expect("sold");
    assert!(
        p.return_unit("s1", Condition::Used, "Berlin-DC2", "platform")
            .is_err(),
        "a return was accepted before settlement"
    );
    p.settle("s1").expect("settled");
    let ledger_before = p.ledger().clone();
    let net_before = p.settlement("s1").map(|s| s.net_after_adjustments());
    p.return_unit("s1", Condition::Used, "Berlin-DC2", "platform")
        .expect("returned");
    assert_ne!(p.ledger(), &ledger_before, "the ledger was left unchanged");
    let u = &p.ledger().units()["u1"];
    assert_eq!(
        (u.condition, u.owner.as_str(), u.state.clone()),
        (Condition::Used, "platform", UnitState::Held)
    );
    assert_eq!(p.ledger().returns().len(), 1);
    assert_eq!(p.ledger().returns()[0].sale, "s1");
    // net 33.5, refund -40, ad valorem fee back +4 => -2.5 (per-unit and
    // per-consignment fees kept).
    assert_eq!(net_before, Some(dec!("33.5")));
    assert_eq!(
        p.settlement("s1").map(|s| s.net_after_adjustments()),
        Some(dec!("-2.5"))
    );
    assert!(
        p.return_unit("s1", Condition::Used, "x", "platform")
            .is_err(),
        "returned twice"
    );
}

// ---- Paper boundary (COMMERCE-012's structural half)

#[test]
fn no_real_merchant_adapter_exists_in_the_commerce_module() {
    let src = include_str!("../src/commerce.rs");
    let body = src.split("#[cfg(test)]").next().unwrap_or(src);
    assert!(body.contains("pub struct SimulatedMerchant"), "premise");
    for forbidden in [
        "std::net",
        "qip_transport",
        "trait MerchantAdapter",
        "TcpStream",
    ] {
        assert!(
            !body.contains(forbidden),
            "commerce.rs mentions {forbidden}"
        );
    }
}

#[test]
fn a_listing_from_a_registered_counterfeit_seller_is_a_counterfeit_even_with_a_clean_gtin() {
    let ls = [
        listing("g", "Acme", "X", "1", Some("1"), "acme"),
        listing("f", "Acme", "X", "1", Some("1"), "  Fakes R Us "),
    ];
    let r = Resolver::new(BTreeSet::new(), set(&["fakes r us"]))
        .resolve(&ls)
        .expect("resolves");
    assert!(
        r["g"].counterfeit_finding.is_none(),
        "premise: the genuine one is clean"
    );
    assert!(r["f"].counterfeit_finding.is_some());
    assert_ne!(r["f"].id, r["g"].id);
}

#[test]
fn a_reserved_unit_is_sold_only_under_its_own_reservation() {
    let mut l = Ledger::default();
    l.apply(received("u1", dec!("1"))).expect("received");
    l.apply(LedgerEvent::Reserved {
        unit: "u1".into(),
        reservation: "r1".into(),
    })
    .expect("reserved");
    let sale = |r: Option<&str>| LedgerEvent::Sold {
        unit: "u1".into(),
        reservation: r.map(String::from),
        sale: "s".into(),
        buyer: "b".into(),
    };
    for wrong in [Some("r2"), None] {
        let before = l.clone();
        assert!(l.apply(sale(wrong)).is_err(), "sold under {wrong:?}");
        assert_eq!(l, before);
    }
    l.apply(sale(Some("r1"))).expect("the holder sells");
}

#[test]
fn availability_that_has_halved_is_a_supply_shock_and_slightly_less_is_not() {
    let kinds = |avail| scout(&[obs(Channel::Retail, dec!("100"), avail, 10)]).len();
    assert_eq!(kinds(5), 1, "exactly half is a shock");
    assert_eq!(kinds(6), 0, "a drop to six of ten is not");
}
