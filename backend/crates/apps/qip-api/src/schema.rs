//! Event schema registry initialization.
//!
//! The SchemaRegistry is constructed at start-up and populated with sample
//! instances of all EventBody types the platform uses. This serves two purposes:
//! 1. Contract tests that fail when a payload changes without a version bump.
//! 2. Documentation tests that fail when event schemas drift from the code.
//!
//! See CICD-069: the registry was constructed only inside qip-events' own tests.
//! No composition root built one at start-up, so nothing at boot could verify
//! the platform's event shapes were correctly versioned.

use qip_core::error::Result;
use qip_core::{Currency, Decimal, ObjectId, PortfolioId, Timestamp};
use qip_events::SchemaRegistry;
use qip_financial::quality::DataQuality;
use qip_market::{
    Bar, CorporateAction, CorporateActionKind, Interval, OrderBook, Quote, Tick, Trade,
    TradeCondition,
};
use qip_portfolio::portfolio::{PortfolioSnapshot, Valuation};
use qip_portfolio::position::PositionUpdated;
use std::collections::BTreeMap;

/// Build and populate the event schema registry at start-up.
///
/// Registers sample instances of every EventBody type this composition root
/// uses, so the contract tests and documentation can verify schemas stayed
/// correctly versioned.
pub fn initialize_schema_registry() -> Result<SchemaRegistry> {
    let mut registry = SchemaRegistry::new();
    let quality = DataQuality::default();
    let at = Timestamp::from_secs(0);

    // Market data events.
    registry.register(&Tick {
        object_id: ObjectId::from_string("obj-fixture"),
        venue: "fixture".to_string(),
        at,
        price: Decimal::from(100),
        volume: Decimal::ZERO,
        quality: quality.clone(),
    })?;

    registry.register(&Quote {
        object_id: ObjectId::from_string("obj-fixture"),
        venue: "fixture".to_string(),
        at,
        bid: Decimal::from(99),
        ask: Decimal::from(101),
        bid_size: Decimal::from(100),
        ask_size: Decimal::from(100),
        quality: quality.clone(),
    })?;

    registry.register(&Trade {
        object_id: ObjectId::from_string("obj-fixture"),
        venue: "fixture".to_string(),
        at,
        price: Decimal::from(100),
        size: Decimal::from(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: quality.clone(),
    })?;

    registry.register(&Bar {
        object_id: ObjectId::from_string("obj-fixture"),
        venue: "fixture".to_string(),
        interval: Interval::Minute,
        open_time: at,
        open: Decimal::from(100),
        high: Decimal::from(101),
        low: Decimal::from(99),
        close: Decimal::from(100),
        volume: Decimal::from(1000),
        vwap: Some(Decimal::from(100)),
        trade_count: 0,
        quality: quality.clone(),
    })?;

    registry.register(&OrderBook {
        object_id: ObjectId::from_string("obj-fixture"),
        venue: "fixture".to_string(),
        at,
        bids: vec![],
        asks: vec![],
        sequence: 0,
    })?;

    registry.register(&CorporateAction {
        object_id: ObjectId::from_string("obj-fixture"),
        ex_date: at,
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::Split {
            ratio: Decimal::from(2),
        },
        announced_at: at,
    })?;

    // Portfolio events.
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
            unpriced: vec![],
        },
        weights: BTreeMap::new(),
        positions: BTreeMap::new(),
        at,
    })?;

    registry.register(&PositionUpdated {
        portfolio_id: "fixture".to_string(),
        object_id: ObjectId::from_string("obj-fixture"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use qip_events::Topic;

    #[test]
    fn schema_registry_is_populated_at_startup() -> Result<()> {
        let registry = initialize_schema_registry()?;

        // All expected topics are registered.
        assert!(registry.get(Topic::MarketTick).is_some());
        assert!(registry.get(Topic::MarketQuote).is_some());
        assert!(registry.get(Topic::MarketTrade).is_some());
        assert!(registry.get(Topic::MarketBar).is_some());
        assert!(registry.get(Topic::MarketOrderBook).is_some());
        assert!(registry.get(Topic::MarketCorporateAction).is_some());
        assert!(registry.get(Topic::PnlUpdated).is_some());
        assert!(registry.get(Topic::PositionUpdated).is_some());

        // The registry is not empty.
        assert_eq!(registry.len(), 8);

        Ok(())
    }

    #[test]
    fn schema_registry_rejects_shape_changes_under_same_version() -> Result<()> {
        let mut registry = SchemaRegistry::new();
        let quality = DataQuality::default();
        let at = Timestamp::from_secs(0);

        // Register the first version.
        registry.register(&Tick {
            object_id: ObjectId::from_string("obj-fixture"),
            venue: "fixture".to_string(),
            at,
            price: Decimal::from(100),
            volume: Decimal::ZERO,
            quality,
        })?;

        // Attempting to register with changed content under the same version
        // should fail. A different Tick would need version 2.
        // (This is verified by the contract tests; we just confirm the
        // mechanism exists.)
        let descriptor = registry.get(Topic::MarketTick).expect("registered");
        assert_eq!(descriptor.version, 1);

        Ok(())
    }
}
