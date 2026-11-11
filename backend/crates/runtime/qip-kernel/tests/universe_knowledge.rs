//! The universe record reaches the knowledge graph (R12-EXPAND-6031).
//!
//! An instrument's catalogue record already names its issuer, sector,
//! country of primary risk and quote currency, and nothing wrote them to the
//! knowledge graph: the kernel added a bare node per instrument and no edge,
//! so the cross-domain synthesis had nothing to walk on a default cycle.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::{Platform, UNIVERSE_SOURCE};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_world_model::relationship::RelationshipKind;
use std::collections::BTreeSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn platform() -> Platform {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                ObjectId::from_string("obj-AAA"),
                "AAA",
                InstrumentType::CommonStock,
                liquidity(),
            )
            .venue("XNYS")
            .issuer("Acme Corp")
            .geography("US")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
    .unwrap()
}

fn bar(symbol: &str, at: Timestamp) -> SensedRecord {
    let px = Decimal::from_int(100);
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(format!("obj-{symbol}")),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: px,
        high: px,
        low: px,
        close: px,
        volume: Decimal::from_int(1_000),
        trade_count: 10,
        vwap: None,
        quality: DataQuality::default(),
    }))
}

/// R12-EXPAND-6031: the first record of an instrument the universe holds
/// writes its issuer, sector, country and quote currency to the knowledge
/// graph, joined by `issues`, `in_industry`, `domiciled_in` and `quoted_in`
/// with the universe as source, and the synthesis then finds a chain across
/// three or more domains through it. An instrument the universe does not
/// hold adds a bare node and nothing else.
///
/// Mutation: drop the `absorb_universe_record` call in
/// `ensure_world_object`.
#[test]
fn an_instruments_universe_record_reaches_the_knowledge_graph_as_entities_and_relations() {
    let mut platform = platform();
    let at = start().saturating_add(Duration::from_days(1));
    platform.observe(vec![bar("AAA", at), bar("ZZZ", at)]);
    let now = at.saturating_add(Duration::from_days(1));

    let world = platform.world();
    let graph = world.graph();
    assert!(
        graph.node("obj-AAA").is_some(),
        "premise: the instrument was recorded"
    );
    let edge = |from: &str, kind: RelationshipKind| -> BTreeSet<String> {
        graph
            .neighbours(from, Some(kind), now, now)
            .into_iter()
            .inspect(|fact| assert_eq!(fact.relationship.source, UNIVERSE_SOURCE))
            .map(|fact| fact.relationship.to.clone())
            .collect()
    };
    assert_eq!(
        edge("company:Acme Corp", RelationshipKind::Issues),
        ["obj-AAA".to_string()].into()
    );
    assert_eq!(
        edge("obj-AAA", RelationshipKind::InIndustry),
        [format!("sector:{}", Sector::InformationTechnology)].into()
    );
    assert_eq!(
        edge("obj-AAA", RelationshipKind::DomiciledIn),
        ["country:US".to_string()].into()
    );
    assert_eq!(
        edge("obj-AAA", RelationshipKind::QuotedIn),
        ["currency:USD".to_string()].into()
    );

    assert!(
        graph.node("obj-ZZZ").is_some(),
        "premise: the unknown instrument was recorded"
    );
    assert!(
        graph.neighbours("obj-ZZZ", None, now, now).is_empty(),
        "an instrument the universe does not hold gained relations"
    );

    let chains = qip_world_model::synthesis::cross_domain_hypotheses(&world, now);
    assert!(
        chains
            .iter()
            .any(|c| c.path.contains(&"obj-AAA".to_string())),
        "no cross-domain chain runs through the instrument: {chains:?}"
    );
}
