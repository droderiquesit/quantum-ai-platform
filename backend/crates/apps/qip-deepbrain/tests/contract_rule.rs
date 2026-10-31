//! The contract rule has a subject on the node's shipped default path.
//!
//! The kernel's rule pack carries `r70-contract-expires-before-horizon`
//! (R12-REASON-3057), fed from a future's or an option's expiry at assembly.
//! It was refuted on audit: no shipped default path held a dated contract —
//! the committed catalogue loader refuses derivatives, the committed universe
//! is common stock, and the deep brain's reference universe built only
//! stocks and a bond — so the rule was on the cycle and its input never was.
//! This suite assembles the platform over the deep brain's own derived
//! universe, the way the node's evolution engine does, and proves a journaled
//! derivation on the demo's future carries the margin the rule reads.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_deepbrain::reference;
use qip_financial::asset_class::InstrumentType;
use qip_financial::extensions::Extension;
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_kernel::symbolic_stage::{self, SymbolicDerivation};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
use qip_observability::Telemetry;
use qip_reasoning_engine::rules::Value;
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// The demo world the node ships with, seeded as the node seeds it.
fn demo() -> SyntheticEnvironment {
    SyntheticEnvironment::demo(
        start(),
        EnvironmentConfig {
            seed: PlatformConfig::default().seed,
            ..EnvironmentConfig::default()
        },
    )
}

/// The bar fixture the kernel's own symbolic suite forms a hypothesis from:
/// a drift with a nine per cent jump two thirds of the way through, stamped
/// on the given instrument at its own venue.
fn bars(object_id: &ObjectId, venue: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object_id.clone(),
                venue: venue.to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).expect("a price"),
                high: Decimal::from_f64(open.max(price) * 1.002).expect("a price"),
                low: Decimal::from_f64(open.min(price) * 0.998).expect("a price"),
                close: Decimal::from_f64(price).expect("a price"),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

/// The derivation REASON journals for `subject` on a platform assembled over
/// `universe` the way the node's evolution engine assembles its own: the
/// kernel's bar fixture is observed on the subject alone and one cycle runs.
///
/// One platform per subject rather than two subjects on one, because REASON
/// works the head of its opportunity queue and nothing leaves that queue
/// except by lapsing: two subjects observed together would have the first
/// reasoned about every cycle and the second never reached, for as many
/// cycles as a test could afford.
fn derivation_on(
    universe: Universe,
    subject: &ObjectId,
    venue: &str,
) -> Result<SymbolicDerivation> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;
    platform.observe(bars(subject, venue, 120));
    let report = platform.run_cycle(start());
    // Premise: the cycle formed a hypothesis to derive from.
    assert!(
        !platform.predictions().is_empty(),
        "no hypothesis was formed on {subject}:\n{}",
        report.summarise()
    );
    let derivations = symbolic_stage::replay(&platform)?;
    derivations
        .iter()
        .find(|derivation| derivation.subject == subject.as_str())
        .cloned()
        .ok_or_else(|| {
            Error::not_found(format!(
                "no derivation on {subject}; derived: {:?}",
                derivations
                    .iter()
                    .map(|d| d.subject.as_str())
                    .collect::<Vec<_>>()
            ))
        })
}

fn expiry_margin_days(derivation: &SymbolicDerivation) -> Option<i64> {
    derivation
        .rules
        .inputs
        .iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("expiry_margin_days", Value::Int(days)) => Some(*days),
            _ => None,
        })
}

#[test]
fn the_deep_brains_demo_universe_carries_a_dated_contract_the_contract_rule_reads() -> Result<()> {
    let environment = demo();
    // Premise one: the shipped demo lists a dated contract at all, with an
    // expiry after its listing — the fact the audit found missing.
    let future = environment
        .instruments()
        .iter()
        .find(|instrument| instrument.contract.is_some())
        .ok_or_else(|| Error::not_found("the demo environment lists no dated contract"))?;
    let terms = future
        .contract
        .as_ref()
        .ok_or_else(|| Error::invalid("the contract found carries no terms"))?;
    assert!(
        terms.expiry > start(),
        "the demo's contract expired at {} before it was listed at {}",
        terms.expiry,
        start()
    );
    let stock = environment
        .instruments()
        .iter()
        .find(|instrument| instrument.symbol == terms.underlying_symbol)
        .ok_or_else(|| Error::not_found("the contract's underlying"))?;

    // Premise two: the derived universe registers it as a future whose
    // extension carries that expiry, which is the field `Platform::new`
    // reads into `contract_expiries`.
    let universe = reference::synthetic_universe(&environment, start())?;
    let record = universe.require(&future.object_id)?;
    assert_eq!(record.instrument_type, InstrumentType::Future);
    match &record.extension {
        Extension::Future(details) => assert_eq!(details.expiry, terms.expiry),
        other => {
            return Err(Error::invalid(format!(
                "the future carries no future terms: {other:?}"
            )));
        }
    }

    let on_future = derivation_on(universe, &future.object_id, &future.venue)?;
    let on_stock = derivation_on(
        reference::synthetic_universe(&environment, start())?,
        &stock.object_id,
        &stock.venue,
    )?;

    // The contrast is what proves the margin is the contract's and not a
    // constant every subject carries: the stock the future settles against,
    // derived the same way, has no expiry to fall short of.
    assert_eq!(
        expiry_margin_days(&on_stock),
        None,
        "the stock's derivation carries an expiry margin: {:?}",
        on_stock.rules.inputs
    );
    let margin = expiry_margin_days(&on_future).ok_or_else(|| {
        Error::not_found(format!(
            "the future's derivation carries no expiry_margin_days; its rule inputs were {:?}",
            on_future.rules.inputs
        ))
    })?;
    // The margin is the kernel's arithmetic on the exchange's own expiry:
    // whole days from the claim's deadline to the expiry, floored.
    let inputs = on_future
        .temporal
        .inputs
        .as_ref()
        .ok_or_else(|| Error::not_found("the derivation's temporal inputs"))?;
    let deadline = inputs.formed_at.saturating_add(inputs.horizon);
    let expected = terms
        .expiry
        .since(deadline)
        .as_nanos()
        .div_euclid(Duration::from_days(1).as_nanos());
    assert_eq!(
        margin, expected,
        "the margin the rule read is not the days from the deadline {deadline} to the expiry {}",
        terms.expiry
    );
    Ok(())
}
