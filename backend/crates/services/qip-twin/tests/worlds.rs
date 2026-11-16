//! Synthetic worlds that vary the named causal dimensions (R12-TICK-3097).

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_twin::futures::{TreeConfig, TwinState, WorldDriver};
use qip_twin::worlds::{DriverKind, KindedDriver, SyntheticWorlds};
use std::collections::{BTreeMap, BTreeSet};

fn at(s: i64) -> Timestamp {
    Timestamp::from_secs(1_791_676_800 + s)
}

fn series() -> BTreeMap<String, Vec<(Timestamp, f64)>> {
    let mut out = BTreeMap::new();
    for (name, base) in [("A", 100.0), ("B", 50.0), ("C", 20.0)] {
        let mids = (0..200)
            .map(|i| {
                let w = ((i * 7919) % 13) as f64 / 13.0 - 0.5;
                (at(i), base * (1.0 + 0.001 * w))
            })
            .collect();
        out.insert(name.to_string(), mids);
    }
    out
}

fn config() -> TreeConfig {
    TreeConfig {
        horizon: Duration::from_secs(60),
        shock_sigmas: vec![1.0, 2.0],
        propagation: vec![0.0, 1.0],
        prune_below: 0.001,
        split_beyond_sigmas: 4.0,
        max_branches: 512,
    }
}

fn kinded(kind: DriverKind, id: &str, instrument: &str, sens: f64) -> KindedDriver {
    KindedDriver {
        kind,
        driver: WorldDriver {
            id: id.into(),
            family: None,
            sensitivities: BTreeMap::from([(instrument.to_string(), sens)]),
            evidence: vec![format!("hypothesis:{id}")],
        },
    }
}

fn all_kinds() -> Vec<KindedDriver> {
    vec![
        kinded(DriverKind::Policy, "pol", "A", 1.5),
        kinded(DriverKind::Weather, "wx", "B", -1.0),
        kinded(DriverKind::Default, "dflt", "C", -2.0),
        kinded(DriverKind::Supply, "sup", "A", 0.5),
        kinded(DriverKind::OwnAction, "own", "B", 1.0),
        kinded(DriverKind::Price, "px", "C", 1.0),
    ]
}

/// Mutation: make `kind_of` return `None` for every branch — no kind grows a
/// branch and `kinds_grown` is empty.
#[test]
fn one_generator_grows_worlds_differing_along_every_named_dimension() -> Result<()> {
    let state = TwinState::fit(&series(), at(199))?;
    let worlds = SyntheticWorlds::grow(&state, &all_kinds(), config())?;
    assert_eq!(
        worlds.kinds_grown(),
        DriverKind::ALL.into_iter().collect::<BTreeSet<_>>(),
        "premise: every kind grew worlds"
    );
    // Each kind's worlds move a different instrument vector from every other's.
    let mut signatures = BTreeSet::new();
    for kind in DriverKind::ALL {
        let branches = worlds.branches_of(kind);
        assert!(!branches.is_empty(), "{kind:?} grew nothing");
        // And within one kind the worlds differ along shock and propagation.
        let shocks: BTreeSet<String> = branches
            .iter()
            .map(|b| format!("{:?}", b.assumptions))
            .collect();
        assert!(shocks.len() > 1, "{kind:?} varied no causal assumption");
        let top = branches
            .iter()
            .max_by(|x, y| x.id.cmp(&y.id))
            .map(|b| format!("{:?}", b.impact_bps))
            .unwrap_or_default();
        assert!(
            signatures.insert(top),
            "{kind:?} duplicated another kind's world"
        );
    }
    Ok(())
}

/// Mutation: drop `has_feed_producer`'s `OwnAction | Price` arm to `true` for
/// all kinds — the hypothesis-only branches reach `feed_backed`.
#[test]
fn a_branch_of_a_kind_no_feed_produces_never_reaches_the_market_facing_accessor() -> Result<()> {
    let state = TwinState::fit(&series(), at(199))?;
    let worlds = SyntheticWorlds::grow(&state, &all_kinds(), config())?;
    let backed = worlds.feed_backed();
    assert!(
        backed.iter().any(|b| b.id.starts_with("px:")),
        "premise: a Price branch is feed-backed"
    );
    assert!(backed.iter().any(|b| b.id == "baseline"));
    for b in &backed {
        if let Some(kind) = worlds.kind_of(&b.id) {
            assert!(kind.has_feed_producer(), "{} of {kind:?} leaked", b.id);
        }
    }
    assert!(
        !backed
            .iter()
            .any(|b| b.id.starts_with("pol:") || b.id.starts_with("wx:"))
    );
    assert!(
        !backed
            .iter()
            .any(|b| b.id.starts_with("dflt:") || b.id.starts_with("sup:"))
    );
    Ok(())
}

/// Mutation: remove the unreached-driver check in `grow` — the driver naming
/// an unknown instrument is admitted with a kind that grew nothing.
#[test]
fn a_declared_kind_that_grows_no_world_is_refused() -> Result<()> {
    let state = TwinState::fit(&series(), at(199))?;
    let err = SyntheticWorlds::grow(
        &state,
        &[
            kinded(DriverKind::Price, "px", "A", 1.0),
            kinded(DriverKind::Weather, "wx", "NOPE", 1.0),
        ],
        config(),
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(err.contains("reaches no instrument"), "got {err:?}");
    let dup = SyntheticWorlds::grow(
        &state,
        &[
            kinded(DriverKind::Weather, "x", "A", 1.0),
            kinded(DriverKind::Policy, "x", "B", 1.0),
        ],
        config(),
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert!(dup.contains("declared twice"), "got {dup:?}");
    Ok(())
}
