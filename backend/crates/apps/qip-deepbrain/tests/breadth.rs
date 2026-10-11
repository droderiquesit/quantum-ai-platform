//! The deep brain's breadth plan, from the variable to a running plane.
//!
//! `qip_deepbrain::breadth::install` is what `main` calls; these tests call
//! it with a lookup standing in for the environment and a platform built the
//! way the kernel's own constructor builds one, then run a cycle — so the
//! assertion is about what the binary's path does, not what a file says.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;

use qip_core::error::Result;
use qip_core::{Context, Timestamp};
use qip_deepbrain::breadth::{PLAN_VARIABLE, install, load};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

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

const PLAN: &str = r#"{
  "market_factory": {
    "permissions": [
      {"authority": "venue", "action": "create", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "rulebook 4.2"},
      {"authority": "product", "action": "create", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "product approval 9"},
      {"authority": "jurisdiction", "action": "create", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "opinion 3"},
      {"authority": "venue", "action": "list", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "rulebook 4.2"},
      {"authority": "product", "action": "list", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "product approval 9"},
      {"authority": "jurisdiction", "action": "list", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "opinion 3"},
      {"authority": "venue", "action": "seed", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "rulebook 4.2"},
      {"authority": "product", "action": "seed", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "product approval 9"},
      {"authority": "jurisdiction", "action": "seed", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "opinion 3"}
    ],
    "markets": [
      {"venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB",
       "reference_price": "100", "half_spread": "0.5", "level_step": "0.25",
       "level_size": "10", "levels": 3, "tolerance": "0.05", "inventory": "30",
       "mark_source": "obj-NEVER-OBSERVED"}
    ]
  },
  "event_markets": {"source": "synthetic", "entity": "desk"}
}"#;

fn plan_file(name: &str, text: &str) -> Result<String> {
    let path = std::env::temp_dir().join(format!(
        "qip-deepbrain-breadth-{}-{name}.json",
        std::process::id()
    ));
    std::fs::write(&path, text)
        .map_err(|error| qip_core::Error::io(format!("cannot write the plan: {error}")))?;
    Ok(path.to_string_lossy().into_owned())
}

fn lookup(map: BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> {
    move |name| map.get(name).cloned()
}

/// Unset is no breadth: nothing installs and the platform's planes stay
/// empty. Mutation: default to an empty plan instead of `None` and the
/// install returns lines.
#[test]
fn an_unset_plan_installs_nothing() -> Result<()> {
    let mut platform = platform()?;
    let none = lookup(BTreeMap::new());
    assert!(load(&none)?.is_none());
    assert!(install(&mut platform, &none, start())?.is_none());
    assert!(platform.market_factory().is_none());
    assert!(platform.event_markets().is_none());
    Ok(())
}

/// Set, the plan reaches the platform the binary runs: the market is seeded
/// on the simulated venue and the event plane lists its market at the first
/// cycle. Mutation: drop the `platform.install_breadth` call from
/// `breadth::install` and both are absent.
#[test]
fn a_plan_file_installs_the_factory_and_the_event_plane_into_the_running_platform() -> Result<()> {
    let path = plan_file("good", PLAN)?;
    let mut platform = platform()?;
    let set = lookup(BTreeMap::from([(PLAN_VARIABLE.to_string(), path)]));
    let lines = install(&mut platform, &set, start())?.expect("a plan was set");
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        platform
            .market_factory()
            .and_then(|stage| stage.factory().market("obj-CREATED"))
            .map(|market| market.stage.as_str()),
        Some("seeded")
    );
    platform.run_cycle(start());
    assert_eq!(
        platform
            .event_markets()
            .map(|plane| plane.markets().count()),
        Some(1)
    );
    Ok(())
}

/// A plan that does not parse, names an unknown section, or is empty stops
/// start-up as a configuration error. Mutation: drop `deny_unknown_fields`
/// from `BreadthPlan` and the misspelt section is silently ignored.
#[test]
fn a_malformed_misspelt_or_empty_plan_is_a_configuration_error() -> Result<()> {
    for (name, text) in [
        ("broken", "{ not json"),
        ("misspelt", r#"{"market_factroy": null}"#),
        ("empty", "{}"),
    ] {
        let path = plan_file(name, text)?;
        let set = lookup(BTreeMap::from([(PLAN_VARIABLE.to_string(), path)]));
        let refused = load(&set).expect_err("a bad plan loaded");
        assert!(
            refused.message().starts_with("configuration:"),
            "{name}: {}",
            refused.message()
        );
    }
    Ok(())
}
