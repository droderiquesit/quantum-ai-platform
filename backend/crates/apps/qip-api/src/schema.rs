//! The event schemas this process serves, checked against a committed table
//! before anything is bound (CICD-069).
//!
//! `qip_events::SchemaRegistry` was constructed only inside `qip-events`' own
//! tests, so no composition root ever compared the shapes it was about to
//! stream against anything. The first version of this module built a
//! registry at start-up and `main` discarded it: start-up compared nothing,
//! and a field removed from `Tick` at the version every consumer already
//! trusted still reached the market streams. Now the registry this build
//! computes is compared, topic by topic, against [`COMMITTED_SCHEMAS`], and a
//! difference stops the process naming what to do — a pod that will not
//! start and says why, rather than one serving a shape nobody versioned.
//!
//! # Why the table lives here and not in `qip-events/schemas.lock.json`
//!
//! That lock covers the nine fabric-bound bodies (ADR 0100 §5) and is judged
//! by CI against the pull request's base (CICD-070). The eight bodies below
//! are the market and portfolio bodies this process streams, none of which
//! is fabric-bound; adding them to that lock would widen what the fabric
//! gate claims to cover. The table here is narrower in one respect: it
//! commits each body's schema id, not its full shape, so a refusal says
//! *that* a body moved and not which field did — the regenerated id in the
//! message is the starting point for that diff.
//!
//! # What a sample can and cannot see
//!
//! The ids are read off sample values, and an `Option` left `None` or a
//! collection left empty serialises to a shape that hides its element type.
//! Every sample below fills each optional field and each collection for that
//! reason. What is still invisible: the other variants of an enum the sample
//! does not carry (only `CorporateActionKind::Split` is sampled), and any
//! field a `#[serde(skip_serializing_if)]` omits on the sampled value.

use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, ObjectId, PortfolioId, Timestamp};
use qip_events::SchemaRegistry;
use qip_financial::quality::DataQuality;
use qip_market::{
    Bar, BookLevel, CorporateAction, CorporateActionKind, Interval, OrderBook, Quote, Side, Tick,
    Trade, TradeCondition,
};
use qip_portfolio::portfolio::{PortfolioSnapshot, Valuation};
use qip_portfolio::position::PositionUpdated;
use std::collections::BTreeMap;

/// One committed row: the topic a body is published on, the version it is
/// published at, the Rust type sampled for it, and the content-derived
/// schema id (`qip_events::event_fabric::schema_id::SchemaId`) of its full
/// recursive shape at that version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommittedSchema {
    pub topic: &'static str,
    pub version: u32,
    pub type_name: &'static str,
    pub schema_id: &'static str,
}

/// The schemas this process is committed to serving.
///
/// Changing a body's shape changes its id here. Do it deliberately: bump the
/// body's `SCHEMA_VERSION` first, then commit the new row — the start-up
/// refusal prints the id the build computed. A row edited to match a changed
/// shape *without* a version bump is the failure this table exists to stop,
/// and it is caught in review, not here, because the table cannot tell the
/// two edits apart.
pub const COMMITTED_SCHEMAS: &[CommittedSchema] = &[
    CommittedSchema {
        topic: "market.bar",
        version: 1,
        type_name: "qip_market::bar::Bar",
        schema_id: "6ffe318591887304f17d6123f10fdd21d332b6fd014e06fce20e5eebce17fb0c",
    },
    CommittedSchema {
        topic: "market.corporate_action",
        version: 1,
        type_name: "qip_market::corporate_action::CorporateAction",
        schema_id: "e3bb17721e8accf2ccb2ccc7670d9e481c2838f999f8d3c74840ac2585da2d39",
    },
    CommittedSchema {
        topic: "market.orderbook",
        version: 1,
        type_name: "qip_market::book::OrderBook",
        schema_id: "c585d7ae9ddba5735ad7d4339f0d12bd43339b2ea8755f72c9d467ac8389724a",
    },
    CommittedSchema {
        topic: "market.quote",
        version: 1,
        type_name: "qip_market::quote::Quote",
        schema_id: "63b1d394e2394072a33900fe9e489805794c2b676bd3114e0214bc5d407bb493",
    },
    CommittedSchema {
        topic: "market.tick",
        version: 1,
        type_name: "qip_market::quote::Tick",
        schema_id: "cff724470b8d827eec4b642a920f61954403faf5e6f2b547e56c2a2007346f2e",
    },
    CommittedSchema {
        topic: "market.trade",
        version: 1,
        type_name: "qip_market::quote::Trade",
        schema_id: "ac46b3c7e990b8cc909aeaa2a38b46033e6fd6347a8b0581901bd96a2b32fb33",
    },
    CommittedSchema {
        topic: "pnl.updated",
        version: 1,
        type_name: "qip_portfolio::portfolio::PortfolioSnapshot",
        schema_id: "95c22b363a917f1616fc0c03681bb8b9fc52a5f49eab96cba314a6a95aa7346d",
    },
    CommittedSchema {
        topic: "position.updated",
        version: 1,
        type_name: "qip_portfolio::position::PositionUpdated",
        schema_id: "3d210eaff1d9d2c3ed23c2393e720cd892ed27c2c1bcd3252fc1694c78057f14",
    },
];

/// Build the registry of the bodies this process streams, from samples.
///
/// A sample is required because `SchemaRegistry` reads a shape off the
/// serialised form; there is no reflection to interrogate instead.
pub fn initialize_schema_registry() -> Result<SchemaRegistry> {
    let mut registry = SchemaRegistry::new();
    let quality = DataQuality::default();
    let at = Timestamp::from_secs(0);
    let object_id = || ObjectId::from_string("obj-fixture");

    registry.register(&Tick {
        object_id: object_id(),
        venue: "fixture".to_string(),
        at,
        capture_time: None,
        price: Decimal::from(100),
        volume: Decimal::ZERO,
        quality: quality.clone(),
    })?;

    registry.register(&Quote {
        object_id: object_id(),
        venue: "fixture".to_string(),
        at,
        capture_time: None,
        bid: Decimal::from(99),
        ask: Decimal::from(101),
        bid_size: Decimal::from(100),
        ask_size: Decimal::from(100),
        quality: quality.clone(),
    })?;

    registry.register(&Trade {
        object_id: object_id(),
        venue: "fixture".to_string(),
        at,
        capture_time: None,
        price: Decimal::from(100),
        size: Decimal::from(10),
        aggressor: Some(Side::Buy),
        condition: TradeCondition::Regular,
        trade_id: Some("trade-fixture".to_string()),
        quality: quality.clone(),
    })?;

    registry.register(&Bar {
        object_id: object_id(),
        venue: "fixture".to_string(),
        interval: Interval::Minute,
        open_time: at,
        open: Decimal::from(100),
        high: Decimal::from(101),
        low: Decimal::from(99),
        close: Decimal::from(100),
        volume: Decimal::from(1000),
        vwap: Some(Decimal::from(100)),
        trade_count: 1,
        quality,
    })?;

    registry.register(&OrderBook {
        object_id: object_id(),
        venue: "fixture".to_string(),
        at,
        bids: vec![BookLevel::new(Decimal::from(99), Decimal::from(100))],
        asks: vec![BookLevel::new(Decimal::from(101), Decimal::from(100))],
        sequence: 1,
    })?;

    registry.register(&CorporateAction {
        object_id: object_id(),
        ex_date: at,
        record_date: Some(at),
        payment_date: Some(at),
        kind: CorporateActionKind::Split {
            ratio: Decimal::from(2),
        },
        announced_at: at,
    })?;

    registry.register(&PortfolioSnapshot {
        portfolio_id: PortfolioId::from_string("pf-fixture"),
        name: "fixture".to_string(),
        base_currency: Currency::USD,
        valuation: Valuation {
            at,
            cash: Decimal::from(500_000),
            position_value: Decimal::from(500_000),
            equity: Decimal::from(1_000_000),
            realised_pnl: Decimal::ZERO,
            unrealised_pnl: Decimal::ZERO,
            gross_exposure: Decimal::from(500_000),
            net_exposure: Decimal::from(500_000),
            leverage: 1.0,
            unpriced: vec!["obj-unpriced".to_string()],
        },
        weights: BTreeMap::from([("obj-fixture".to_string(), 0.5)]),
        positions: BTreeMap::from([("obj-fixture".to_string(), Decimal::from(100))]),
        at,
    })?;

    registry.register(&PositionUpdated {
        portfolio_id: "fixture".to_string(),
        object_id: object_id(),
        symbol: "fixture".to_string(),
        quantity: Decimal::from(100),
        average_price: Decimal::from(100),
        market_value: Decimal::from(10_000),
        realised_pnl: Decimal::ZERO,
        unrealised_pnl: Decimal::ZERO,
        at,
    })?;

    Ok(registry)
}

/// Refuse unless `registry` and `committed` describe exactly the same
/// schemas: the same topics, each at the same version, sampled from the same
/// type, hashing to the same id.
///
/// Both directions are checked. A registered body with no committed row is a
/// schema nobody versioned; a committed row with no registered body is a
/// commitment this build silently stopped keeping. An empty registry is
/// refused too, because comparing nothing with nothing is the vacuous pass
/// the discarded registry used to be.
pub fn verify_against(registry: &SchemaRegistry, committed: &[CommittedSchema]) -> Result<()> {
    if registry.is_empty() {
        return Err(Error::schema(
            "schema check: the registry is empty, so start-up would compare nothing; \
             register the bodies this process serves in qip_api::schema::initialize_schema_registry",
        ));
    }

    for row in committed {
        let Some(built) = registry.iter().find(|d| d.topic.name() == row.topic) else {
            return Err(Error::schema(format!(
                "schema check: topic {} is committed at version {} but this build registers no \
                 body for it; register its body in qip_api::schema::initialize_schema_registry, \
                 or remove the row from COMMITTED_SCHEMAS if the topic is deliberately retired",
                row.topic, row.version
            )));
        };
        if built.type_name != row.type_name {
            return Err(Error::schema(format!(
                "schema check: topic {} is committed against {} but this build samples {}; \
                 a topic reassigned to another type is a new schema — give it a new row",
                row.topic, row.type_name, built.type_name
            )));
        }
        if built.version < row.version {
            return Err(Error::schema(format!(
                "schema check: topic {} is committed at version {} but {} declares version {}; \
                 that is a rollback consumers on version {} cannot read — restore the version",
                row.topic, row.version, built.type_name, built.version, row.version
            )));
        }
        if built.version > row.version {
            return Err(Error::schema(format!(
                "schema check: {} (topic {}) is at version {} but COMMITTED_SCHEMAS holds \
                 version {}; commit the new row (version {}, schema id {}) in \
                 qip-api/src/schema.rs",
                built.type_name,
                row.topic,
                built.version,
                row.version,
                built.version,
                built.schema_id
            )));
        }
        if built.schema_id.as_str() != row.schema_id {
            return Err(Error::schema(format!(
                "schema check: {} (topic {}) changed shape at version {} without a version bump: \
                 committed id {}, built id {}. Revert the change, or bump {}'s SCHEMA_VERSION and \
                 commit the new row in qip-api/src/schema.rs",
                built.type_name,
                row.topic,
                row.version,
                row.schema_id,
                built.schema_id,
                built.type_name
            )));
        }
    }

    for built in registry.iter() {
        if !committed.iter().any(|row| row.topic == built.topic.name()) {
            return Err(Error::schema(format!(
                "schema check: {} (topic {}, version {}) is registered but has no row in \
                 COMMITTED_SCHEMAS, so nothing says which shape consumers were promised; \
                 commit its row (schema id {}) in qip-api/src/schema.rs",
                built.type_name,
                built.topic.name(),
                built.version,
                built.schema_id
            )));
        }
    }

    Ok(())
}

/// The registry this build computes, admitted only if it matches
/// [`COMMITTED_SCHEMAS`]. `main` calls this before storage is opened or a
/// port bound, and a refusal stops the process.
pub fn verified_schema_registry() -> Result<SchemaRegistry> {
    let registry = initialize_schema_registry()?;
    verify_against(&registry, COMMITTED_SCHEMAS)?;
    Ok(registry)
}

/// The start-up banner line: how many topics were checked, and the
/// whole-surface fingerprint an operator can compare across two deployments.
pub fn banner(registry: &SchemaRegistry) -> String {
    format!(
        "{} topic(s) match the committed table; surface {}",
        registry.len(),
        registry.fingerprint()
    )
}

#[cfg(test)]
mod tests {
    // These tests return `Result` and report a failed property as an `Err`
    // through `ensure`, never through `assert!`: the workspace denies
    // `panic_in_result_fn`, and this module is not exempted from it.
    use super::*;
    use qip_events::Topic;
    use qip_events::event_fabric::schema_id::{SchemaId, Shape};

    fn ensure(condition: bool, what: &str) -> Result<()> {
        if condition {
            Ok(())
        } else {
            Err(Error::invalid(format!("test property failed: {what}")))
        }
    }

    /// The refusal `verify_against` returned, or an `Err` saying it admitted.
    fn refusal(registry: &SchemaRegistry, committed: &[CommittedSchema]) -> Result<String> {
        match verify_against(registry, committed) {
            Ok(()) => Err(Error::invalid(
                "test property failed: verify_against admitted a registry it should refuse",
            )),
            Err(error) => Ok(error.message().to_string()),
        }
    }

    /// The registry with the `market.tick` descriptor replaced by one whose
    /// shape gained `extra` and whose version is `version` — what a build
    /// carries after a field is added to `Tick`, with or without the bump.
    fn with_tick_drifted(version: u32) -> Result<SchemaRegistry> {
        let built = initialize_schema_registry()?;
        let mut tick = built
            .get(Topic::MarketTick)
            .cloned()
            .ok_or_else(|| Error::invalid("premise: market.tick is registered"))?;
        let Shape::Object(mut fields) = tick.shape.clone() else {
            return Err(Error::invalid("premise: a Tick serialises to an object"));
        };
        fields.insert("extra".to_string(), Shape::Number);
        tick.shape = Shape::Object(fields);
        tick.version = version;
        tick.schema_id = SchemaId::new(Topic::MarketTick.name(), version, &tick.shape);

        let mut drifted = SchemaRegistry::new();
        for descriptor in built.iter() {
            if descriptor.topic == Topic::MarketTick {
                drifted.admit(tick.clone())?;
            } else {
                drifted.admit(descriptor.clone())?;
            }
        }
        Ok(drifted)
    }

    #[test]
    fn schema_registry_is_populated_at_startup() -> Result<()> {
        let registry = initialize_schema_registry()?;
        for topic in [
            Topic::MarketTick,
            Topic::MarketQuote,
            Topic::MarketTrade,
            Topic::MarketBar,
            Topic::MarketOrderBook,
            Topic::MarketCorporateAction,
            Topic::PnlUpdated,
            Topic::PositionUpdated,
        ] {
            ensure(registry.get(topic).is_some(), topic.name())?;
        }
        ensure(registry.len() == 8, "exactly the eight sampled bodies")
    }

    /// The admitting half. Without it the drift test below would also pass
    /// against a check that refuses everything — a process that never starts.
    #[test]
    fn the_schemas_this_build_registers_match_the_committed_table_and_are_admitted() -> Result<()> {
        ensure(
            !COMMITTED_SCHEMAS.is_empty(),
            "premise: the table is non-empty",
        )?;
        let registry = verified_schema_registry()?;
        ensure(
            registry.len() == COMMITTED_SCHEMAS.len(),
            "one registered body per committed row",
        )
    }

    /// The failure the discarded registry let through: `Tick` gains a field
    /// at the version consumers already trust, and the process still serves.
    #[test]
    fn a_body_whose_shape_drifted_without_a_version_bump_is_refused_at_startup() -> Result<()> {
        let drifted = with_tick_drifted(1)?;
        let message = refusal(&drifted, COMMITTED_SCHEMAS)?;
        ensure(
            message.contains("(topic market.tick)"),
            "the refusal names the drifted topic",
        )?;
        ensure(
            message.contains("without a version bump"),
            "the refusal says the version was not bumped",
        )?;
        ensure(
            message.contains("bump qip_market::quote::Tick's SCHEMA_VERSION"),
            "the refusal names what to do",
        )
    }

    /// A deliberate bump is still refused until its row is committed: the
    /// table, not the type, is what consumers were promised.
    #[test]
    fn a_version_bump_with_no_committed_row_is_refused_naming_the_row_to_commit() -> Result<()> {
        let bumped = with_tick_drifted(2)?;
        let message = refusal(&bumped, COMMITTED_SCHEMAS)?;
        ensure(
            message.contains("is at version 2 but COMMITTED_SCHEMAS holds version 1"),
            "the refusal names both versions",
        )?;
        let built = bumped
            .get(Topic::MarketTick)
            .ok_or_else(|| Error::invalid("premise: market.tick is registered"))?;
        ensure(
            message.contains(&format!("schema id {})", built.schema_id)),
            "the refusal carries the id to commit",
        )
    }

    /// Both directions: a body registered with no row, and a row with no body.
    #[test]
    fn a_body_without_a_row_and_a_row_without_a_body_are_each_refused() -> Result<()> {
        let registry = initialize_schema_registry()?;

        let without_tick: Vec<CommittedSchema> = COMMITTED_SCHEMAS
            .iter()
            .copied()
            .filter(|row| row.topic != "market.tick")
            .collect();
        ensure(
            without_tick.len() + 1 == COMMITTED_SCHEMAS.len(),
            "premise: exactly one row was removed",
        )?;
        let message = refusal(&registry, &without_tick)?;
        ensure(
            message.contains("(topic market.tick, version 1) is registered but has no row"),
            "an unversioned registered body is refused",
        )?;

        let mut with_extra = COMMITTED_SCHEMAS.to_vec();
        with_extra.push(CommittedSchema {
            topic: "market.retired",
            version: 1,
            type_name: "qip_market::Retired",
            schema_id: "0",
        });
        let message = refusal(&registry, &with_extra)?;
        ensure(
            message.contains(
                "topic market.retired is committed at version 1 but this build \
                 registers no body",
            ),
            "a committed row nobody registers is refused",
        )
    }
}
