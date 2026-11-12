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
    demo_at(start())
}

/// The demo world as the node builds it at `at`.
fn demo_at(at: Timestamp) -> SyntheticEnvironment {
    SyntheticEnvironment::demo(
        at,
        EnvironmentConfig {
            seed: PlatformConfig::default().seed,
            ..EnvironmentConfig::default()
        },
    )
}

/// The bar fixture the kernel's own symbolic suite forms a hypothesis from:
/// a drift with a nine per cent jump two thirds of the way through, stamped
/// on the given instrument at its own venue.
fn bars(object_id: &ObjectId, venue: &str, count: usize, now: Timestamp) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = now.saturating_sub(Duration::from_days((count - i) as i64));
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
    derivation_at(universe, subject, venue, start())
}

fn derivation_at(
    universe: Universe,
    subject: &ObjectId,
    venue: &str,
    now: Timestamp,
) -> Result<SymbolicDerivation> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(now, config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;
    platform.observe(bars(subject, venue, 120, now));
    let report = platform.run_cycle(now);
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

/// The committed catalogue, read the way `main.rs`'s `load_universe` reads it.
/// After the catalogue's `as_of`, because the loader refuses a record
/// ingested before it happened.
fn node_start() -> Timestamp {
    Timestamp::from_secs(1_793_000_000)
}

fn committed_catalogue() -> Result<Universe> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../data/datasets/universe.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| Error::io(format!("{}: {error}", path.display())))?;
    Ok(qip_financial::catalogue::load(&text, node_start())?.universe)
}

/// The refuted half of R12-REASON-3057: the node's `main.rs` built its
/// platform over the committed catalogue alone, so on the shipped synthetic
/// arm the demo future was observed and listed nowhere, and the rule had no
/// expiry to read. This assembles the platform exactly as `main.rs` now does
/// on that arm — the committed catalogue through
/// `reference::platform_universe` with the demo exchange — and proves the
/// derivation on the future carries the margin.
#[test]
fn the_nodes_synthetic_arm_assembles_the_platform_over_the_catalogue_and_the_demo_future_so_the_rule_reads_its_expiry()
-> Result<()> {
    let environment = demo_at(node_start());
    let future = environment
        .instruments()
        .iter()
        .find(|instrument| instrument.contract.is_some())
        .ok_or_else(|| Error::not_found("the demo environment lists no dated contract"))?;
    let catalogue = committed_catalogue()?;
    // Premise: the committed catalogue does not list the future, which is
    // the defect — merging must be what puts it there.
    assert!(
        catalogue.get(&future.object_id).is_none(),
        "the committed catalogue already lists {}; this test would prove nothing",
        future.object_id
    );
    let catalogue_ids: Vec<ObjectId> = catalogue.ids().collect();
    assert!(
        !catalogue_ids.is_empty(),
        "the committed catalogue is empty"
    );

    let seed = PlatformConfig::default().seed;
    let step = EnvironmentConfig::default().step;
    // The tape and replay arms take the catalogue untouched, origin kept,
    // and build no demo exchange.
    for (tape, replay) in [(true, false), (false, true)] {
        let (untouched, arm) = reference::node_assembly(
            committed_catalogue()?,
            tape,
            replay,
            seed,
            step,
            node_start(),
        )?;
        assert!(
            arm.is_none(),
            "tape={tape} replay={replay} built a demo exchange"
        );
        assert!(untouched.get(&future.object_id).is_none());
        assert!(
            untouched.origin().is_some(),
            "the catalogue arm lost its origin"
        );
    }

    // The shipped default: neither a tape nor a replay.
    let (universe, arm) =
        reference::node_assembly(catalogue, false, false, seed, step, node_start())?;
    let arm = arm.ok_or_else(|| Error::not_found("the synthetic arm built no demo exchange"))?;
    // The exchange the engine is handed lists the same future the platform
    // now holds, so the bars it emits land on a known instrument.
    assert!(
        arm.instruments()
            .iter()
            .any(|instrument| instrument.object_id == future.object_id),
        "the node's demo exchange does not price the future the universe lists"
    );
    // Every catalogue record survives, as the catalogue wrote it.
    let reread = committed_catalogue()?;
    for id in &catalogue_ids {
        assert_eq!(
            universe.get(id),
            reread.get(id),
            "the catalogue's record of {id} was replaced by the merge"
        );
    }
    assert_eq!(
        universe.require(&future.object_id)?.instrument_type,
        InstrumentType::Future
    );

    let on_future = derivation_at(universe, &future.object_id, &future.venue, node_start())?;
    let margin = expiry_margin_days(&on_future);
    assert!(
        margin.is_some(),
        "the future's derivation on the node's assembly carries no expiry_margin_days; its \
         rule inputs were {:?}",
        on_future.rules.inputs
    );
    Ok(())
}

/// The binary calls the assembly the suite above drives, hands the platform
/// the universe it returns, and builds the evolution engine over the same
/// demo exchange — so the test's arm is the shipped arm and not a copy of it.
/// A binary cannot be called from a test; this pins the three seams by their
/// delimited call text, and fails if `main.rs` goes back to the catalogue
/// alone or to a second demo exchange.
#[test]
fn the_deep_brains_main_assembles_its_platform_through_node_assembly_and_one_demo_exchange() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .expect("main.rs");
    let squeeze = |text: &str| text.split_whitespace().collect::<String>();
    let main = squeeze(&main);
    for seam in [
        "let(platform_universe,mutsynthetic_arm)=qip_deepbrain::reference::node_assembly(\
         catalogue.universe,tape_feed.is_some(),replay_path.is_some(),",
        "Platform::with_model_provider(platform_config,context,telemetry,platform_universe,",
        "letenvironment=synthetic_arm.take()",
        "EvolutionEngine::over_synthetic(evolution_config,environment,",
    ] {
        assert!(main.contains(seam), "main.rs no longer carries `{seam}`");
    }
    assert_eq!(
        main.matches("SyntheticEnvironment::demo(").count(),
        0,
        "main.rs builds a demo exchange of its own beside node_assembly's"
    );
}
