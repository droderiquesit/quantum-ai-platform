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

/// The object whose observed price marks the created market, so the market
/// survives its economic review and only the legal check can withdraw it.
const MARK: &str = "OBJ00000000000000000MARK";

fn plan_without(revoked: Option<(&str, &str)>) -> String {
    let mut permissions = Vec::new();
    for action in ["create", "list", "seed"] {
        for authority in ["venue", "product", "jurisdiction"] {
            if revoked == Some((authority, action)) {
                continue;
            }
            permissions.push(format!(
                r#"{{"authority": "{authority}", "action": "{action}", "venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB", "reference": "{authority} approval"}}"#
            ));
        }
    }
    format!(
        r#"{{
  "market_factory": {{
    "permissions": [{}],
    "markets": [
      {{"venue": "simulated-venue", "product": "obj-CREATED", "jurisdiction": "GB",
       "reference_price": "100", "half_spread": "0.5", "level_step": "0.25",
       "level_size": "10", "levels": 3, "tolerance": "0.05", "inventory": "30",
       "mark_source": "{MARK}"}}
    ]
  }}
}}"#,
        permissions.join(",")
    )
}

fn mark(platform: &mut Platform, at: Timestamp) {
    use qip_core::{Decimal, ObjectId};
    use qip_financial::quality::DataQuality;
    use qip_market::bar::{Bar, Interval};
    use qip_market_ingestion::adapter::SensedRecord;
    let bar = Bar {
        object_id: ObjectId::from_string(MARK),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at.saturating_sub(qip_core::Duration::from_days(1)),
        open: Decimal::from_f64(100.0).unwrap(),
        high: Decimal::from_f64(100.2).unwrap(),
        low: Decimal::from_f64(99.8).unwrap(),
        close: Decimal::from_f64(100.0).unwrap(),
        volume: Decimal::from_f64(1_000_000.0).unwrap(),
        trade_count: 5_000,
        vwap: Decimal::from_f64(100.0),
        quality: DataQuality::default(),
    };
    platform.observe(vec![SensedRecord::Bar(Box::new(bar))]);
}

fn stage(platform: &Platform) -> Option<&'static str> {
    platform
        .market_factory()
        .and_then(|stage| stage.factory().market("obj-CREATED"))
        .map(|market| market.stage.as_str())
}

/// R12-EXEC-5081 from the binary's path: an operator deletes the legal
/// opinion a created market's seeding rests on from the plan file the node
/// was started with, and the next cycle the run loop drives withdraws the
/// market. Before this, permissions were fixed at install and no binary
/// could revoke one, so the monitor's legal check could not fire.
///
/// Calls what `node::run_with` calls — `PermissionWatch::refresh` then
/// `node::step` — with the watch built by `from_lookup`, as `main` builds it.
///
/// Mutations: (1) make `refresh` return `Ok(None)` without applying — the
/// market stays seeded after the revocation; (2) make `revoke_all` skip
/// `revoke_market_permissions` — the unreadable-plan half stays seeded.
#[test]
fn a_permission_revoked_in_the_plan_file_withdraws_the_created_market_at_the_next_cycle()
-> Result<()> {
    use qip_deepbrain::breadth::PermissionWatch;
    use qip_deepbrain::node::step;
    let interval = qip_core::Duration::from_secs(300);

    for unreadable in [false, true] {
        let path = plan_file(&format!("revocable-{unreadable}"), &plan_without(None))?;
        let set = lookup(BTreeMap::from([(PLAN_VARIABLE.to_string(), path.clone())]));
        let mut platform = platform()?;
        install(&mut platform, &set, start())?.expect("a plan was set");
        let mut watch = PermissionWatch::from_lookup(&set).expect("a plan is named");

        mark(&mut platform, start());
        assert_eq!(watch.refresh(&mut platform)?, Some(9));
        let _ = step(&mut platform, start(), interval);
        assert_eq!(
            stage(&platform),
            Some("seeded"),
            "premise: a marked, fully permitted market survives its review"
        );
        assert_eq!(
            watch.refresh(&mut platform)?,
            None,
            "premise: an unchanged register is not re-applied"
        );

        let later = start().saturating_add(qip_core::Duration::from_hours(1));
        if unreadable {
            std::fs::write(&path, "{ not json").unwrap();
            let refused = watch
                .refresh(&mut platform)
                .expect_err("an unreadable register was accepted");
            assert!(
                refused.message().contains("revoked"),
                "{}",
                refused.message()
            );
        } else {
            std::fs::write(&path, plan_without(Some(("jurisdiction", "seed")))).unwrap();
            assert_eq!(watch.refresh(&mut platform)?, Some(8));
        }
        mark(&mut platform, later);
        let _ = step(&mut platform, later, interval);
        assert_eq!(
            stage(&platform),
            Some("withdrawn"),
            "unreadable={unreadable}: a market kept trading on a lapsed permission"
        );
    }
    Ok(())
}
