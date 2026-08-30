//! Choosing a transport by configuration, and being told what you chose.
//!
//! The decoupling this file is about is not "the trait exists" — it always
//! did. It is that a deployment can change which transport carries its events
//! without editing the code that publishes them, and that the change is
//! refused rather than silently downgraded when the new transport cannot keep
//! the promises the old one made.
//!
//! Two failures are being designed against, and both have the same shape:
//! something quietly working instead of the thing that was asked for.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{at, hot_tick, warm_note};
use qip_core::error::Result;
use qip_streaming::registry::{self, TransportConfig, TransportKind};
use qip_streaming::routing::{RoutingClass, TransportPath};

fn local() -> TransportConfig {
    TransportConfig::new(TransportKind::Local, "local-under-test")
}

fn durable() -> TransportConfig {
    TransportConfig::new(TransportKind::Durable, "durable-under-test")
}

// --- the same code, two transports ------------------------------------------

#[test]
fn one_scenario_runs_through_two_transports_chosen_only_by_a_name() -> Result<()> {
    // The publishing code below names no concrete transport. If the
    // abstraction leaks, this is where it shows: the same six lines have to
    // work for both, and the receipts have to agree about what happened.
    // Each routing class has exactly one legal path, so the two runs carry the
    // class each transport is for. What is being demonstrated is that this
    // closure names no transport type — it receives one and publishes through
    // it, and swapping which one is a change to the configuration above.
    let run =
        |config: &TransportConfig, class: RoutingClass| -> Result<(usize, TransportPath, String)> {
            let mut publisher = registry::select_for(config, class)?;
            let mut receipts = Vec::new();
            for index in 0..3 {
                let envelope = match class {
                    RoutingClass::Hot => {
                        hot_tick(index as u64 + 1, at(index * 10), at(index * 10 + 1))?
                    }
                    _ => warm_note(&format!("N{index}"), at(index * 10), at(index * 10 + 1))?,
                };
                receipts.push(publisher.publish(envelope, at(index * 10 + 2))?);
            }
            let descriptor = publisher.descriptor();
            Ok((receipts.len(), descriptor.path, descriptor.name))
        };

    let (local_count, local_path, local_name) = run(&local(), RoutingClass::Hot)?;
    let (durable_count, durable_path, durable_name) = run(&durable(), RoutingClass::Warm)?;

    // What is identical: the caller's experience of publishing.
    assert_eq!(
        local_count, 3,
        "the local transport accepted a different count"
    );
    assert_eq!(
        durable_count, 3,
        "the durable transport accepted a different count"
    );
    assert_eq!(local_name, "local-under-test");
    assert_eq!(durable_name, "durable-under-test");

    // What differs, and must: the guarantee. An abstraction that hid this
    // would be worse than none, because the whole reason to choose is that
    // they are not the same.
    assert_eq!(local_path, TransportPath::Local);
    assert_eq!(durable_path, TransportPath::Durable);
    Ok(())
}

#[test]
fn the_advertised_guarantees_match_what_the_transport_reports() -> Result<()> {
    // A registry whose advertised guarantees drifted from the built
    // transport's own descriptor would be worse than no registry: a caller
    // would choose on the advertisement and live with the reality.
    for config in [local(), durable()] {
        let advertised = config.kind.guarantees();
        let built = registry::publisher(&config)?.descriptor();
        assert_eq!(
            advertised.path,
            built.path,
            "{} advertises the {} path and reports the {}",
            config.kind.as_str(),
            advertised.path.as_str(),
            built.path.as_str()
        );
        assert_eq!(
            advertised.durable,
            built.durable,
            "{} advertises durable={} and reports durable={}",
            config.kind.as_str(),
            advertised.durable,
            built.durable
        );
    }
    Ok(())
}

// --- refusing, rather than substituting -------------------------------------

#[test]
fn selecting_an_unavailable_transport_fails_and_never_falls_back() {
    // The failure this exists to prevent: a service configured for a bus it
    // cannot reach, quietly served by one it can, passing every smoke test and
    // publishing to nothing.
    let config = TransportConfig::new(TransportKind::PubSub, "pubsub-under-test");
    let error = registry::publisher(&config).expect_err("an unusable transport was constructed");

    assert_eq!(error.code(), "unavailable", "{}", error.message());
    let message = error.message();
    assert!(
        message.contains("gRPC") && message.contains("TLS"),
        "the refusal does not name what is missing: {message}"
    );
    assert!(
        message.contains("smoke test"),
        "the refusal does not say why substitution would be worse: {message}"
    );

    // And the subscriber half refuses identically. A transport that could be
    // read from but not written to would be a stranger failure than either.
    assert!(registry::subscriber(&config).is_err());
}

#[test]
fn an_unknown_transport_name_lists_the_ones_that_exist() {
    let error = TransportKind::parse("kafka").expect_err("an unknown name was accepted");
    let message = error.message();
    for known in ["local", "durable", "mesh", "pubsub"] {
        assert!(
            message.contains(known),
            "the error does not offer {known}: {message}"
        );
    }
}

#[test]
fn the_mesh_needs_a_peer_and_says_so_rather_than_defaulting_to_one() {
    // A default peer would be a guess about topology, and the guess would be
    // wrong in exactly the deployment nobody tested.
    let config = TransportConfig::new(TransportKind::Mesh, "mesh-under-test");
    let error = registry::publisher(&config).expect_err("a peerless mesh was constructed");
    assert!(
        error.message().contains("peer"),
        "the refusal does not name the missing peer: {}",
        error.message()
    );
}

// --- guarantees are readable before anything is built -----------------------

#[test]
fn a_lossy_transport_is_refused_for_an_event_that_must_not_be_lost() -> Result<()> {
    // The local queue is bounded and lossy under overload by design. A Warm or
    // Cold event must not be lost. Pairing them is a configuration error and
    // is caught here rather than discovered later as missing data.
    let error = registry::select_for(&local(), RoutingClass::Warm)
        .expect_err("a warm event was routed onto a lossy transport");
    assert_eq!(error.code(), "denied", "{}", error.message());
    assert!(
        error.message().contains("drops its oldest"),
        "the refusal does not say why: {}",
        error.message()
    );
    assert!(registry::select_for(&local(), RoutingClass::Cold).is_err());

    // The durable path carries what routes to it, and only that.
    for class in [RoutingClass::Warm, RoutingClass::Cold] {
        assert!(
            registry::select_for(&durable(), class).is_ok(),
            "the durable transport refused a {} event",
            class.as_str()
        );
    }

    // The inverse is refused too, and this is the half I got wrong first. A
    // hot event on the durable path is not merely wasteful: an append and a
    // hash on the venue-critical path is exactly the latency that class exists
    // to avoid. The transports already refused it at publish time; the
    // registry now refuses it at configuration time, for the same reason.
    let error = registry::select_for(&durable(), RoutingClass::Hot)
        .expect_err("a hot event was routed onto the durable path");
    assert!(
        error.message().contains("latency"),
        "the refusal does not say why: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn what_a_transport_promises_is_readable_without_building_one() {
    // The point of the descriptor being data: a caller can refuse a transport
    // before paying to construct it, and a deployment can be audited from
    // configuration alone.
    for kind in TransportKind::ALL {
        let guarantees = kind.guarantees();
        assert_eq!(guarantees.name, kind.as_str());
        assert_eq!(
            guarantees.available,
            kind != TransportKind::PubSub,
            "{} reports the wrong availability",
            kind.as_str()
        );
        assert_eq!(
            guarantees.production_requirement.is_some(),
            !guarantees.available,
            "{} either claims to work and lists a requirement, or does neither",
            kind.as_str()
        );
    }

    // The mesh is the one whose path and durability disagree, and that is not
    // an oversight: the path says which routing class may travel here, and
    // `durable` says what survives a restart. Its queues are in memory.
    let mesh = TransportKind::Mesh.guarantees();
    assert_eq!(mesh.path, TransportPath::Durable);
    assert!(
        !mesh.durable,
        "the mesh claims to survive a restart; its queue, inbox and dead letters are in memory"
    );
}
