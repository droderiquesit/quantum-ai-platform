#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

//! EXPAND-051, for the two grant models this crate holds: an agent's
//! capability set and a tool's scope. What a grant can execute is exactly what
//! it names, and no research permission, however many, adds to it.
//!
//! The failure prevented is implication. A rule such as "an agent that may
//! propose a trade may also submit it" is one convenient line in `require`,
//! and it turns every broad research grant into an execution grant nobody
//! wrote down. The strategy ladder and the edge cell's capital envelope hold
//! execution authority by other mechanisms and are not covered here.

use qip_agents::tools::{ToolKind, ToolPermission, ToolRegistry};
use qip_agents::{Capability, CapabilitySet};
use std::collections::BTreeSet;

/// A fixed-seed generator, so a failing grant replays.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// Everything the set lets its holder do to the market.
fn executable(set: &CapabilitySet) -> BTreeSet<Capability> {
    Capability::all()
        .into_iter()
        .filter(|c| c.touches_market() && set.require(*c, "generated").is_ok())
        .collect()
}

#[test]
fn what_a_generated_grant_can_execute_is_exactly_what_it_names_and_no_research_permission_adds_to_it()
 {
    let all = Capability::all();
    // Observation, computation and production: sensitivity below authority.
    let research: Vec<Capability> = all
        .iter()
        .copied()
        .filter(|c| c.sensitivity() < 3)
        .collect();
    let market: BTreeSet<Capability> = all
        .iter()
        .copied()
        .filter(Capability::touches_market)
        .collect();
    // The premise: both kinds exist, and they do not overlap.
    assert_eq!(market.len(), 3);
    assert!(research.len() > 15);
    assert!(research.iter().all(|c| !market.contains(c)));

    let mut rng = Lcg(51);
    let mut with_authority = 0;
    for _ in 0..500 {
        let bits = rng.next();
        let named: BTreeSet<Capability> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| bits >> i & 1 == 1)
            .map(|(_, c)| *c)
            .collect();
        let set = CapabilitySet::of(named.iter().copied());

        // Exactly what was named, over every capability there is.
        for capability in &all {
            assert_eq!(
                set.require(*capability, "generated").is_ok(),
                named.contains(capability),
                "{capability} against a grant of {named:?}"
            );
        }
        let before = executable(&set);
        assert_eq!(before, named.intersection(&market).copied().collect());
        with_authority += usize::from(!before.is_empty());

        // Every research permission at once is the broadest research grant
        // there is, and it executes nothing more.
        let mut widened = set.clone();
        for permission in &research {
            widened = widened.with(*permission);
            assert_eq!(
                executable(&widened),
                before,
                "adding {permission} changed what the grant can execute"
            );
        }
    }
    // The premise: the generator produced grants with execution authority
    // and grants without it.
    assert!(with_authority > 100 && with_authority < 500);

    // A tool's scope is the same kind of grant. Read is its research
    // permission; granting it again, or registering more tools, widens nothing.
    for round in 0..200 {
        let mut tools = ToolRegistry::new();
        tools.register("subject", ToolKind::Connector).unwrap();
        let mut granted = BTreeSet::from([ToolPermission::Read]);
        for step in 0..(rng.next() % 4) {
            let grant = [
                ToolPermission::Read,
                ToolPermission::Write,
                ToolPermission::LeaveSandbox,
            ][usize::try_from(rng.next() % 3).unwrap()];
            tools.promote("subject", grant, "evaluation").unwrap();
            granted.insert(grant);
            tools
                .register(&format!("bystander-{round}-{step}"), ToolKind::Parser)
                .unwrap();
        }
        for permission in [
            ToolPermission::Read,
            ToolPermission::Write,
            ToolPermission::LeaveSandbox,
        ] {
            assert_eq!(
                tools.authorise("subject", permission).is_ok(),
                granted.contains(&permission),
                "{permission:?} against a scope of {granted:?}"
            );
        }
    }
}
