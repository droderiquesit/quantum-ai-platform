//! SEC-001: what flows down to a cell stays signed after a peer channel
//! exists beside the downward path.
//!
//! v11.6 relaxed "policy down, outcomes up only" by adding peer messaging,
//! opportunity tokens and reservations between cells. The relaxation is about
//! what *else* may flow. The failure it invites is the obvious shortcut: once
//! a node has a second inbound channel, a policy, a halt or a grant arriving
//! on it is one `match` arm away from being applied, and a peer — or whoever
//! sits between two peers — is then able to install policy on a node that
//! trades.
//!
//! Two things hold that line and each crate's own tests can see only one of
//! them. `qip-mesh` knows a peer frame naming a downward kind is refused;
//! `qip-edge` knows `Cell::apply_policy` takes a `VerifiedPolicy`. Neither
//! can see that the second is the *only* way in, or that the first holds for
//! a payload the centre really signed. This suite reaches both ends.
//!
//! What it does not claim. The signature is an HMAC under a key the cell
//! shares with the centre (ADR 0043 §2), so a cell that can verify a payload
//! can also mint one; and the peer protocol has no transport yet (MESH-038),
//! so "a peer channel live on the node" means its endpoint holding an epoch,
//! not a socket. SEC-001's row says both.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::read;
use qip_contracts::policy::{HaltCommand, PolicyPayload};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::policy::{VerifiedHalt, VerifiedPolicy};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_mesh::peer::{EpochDraft, EpochLeg, PeerEndpoint, PeerLimits, PeerMessage, encode};

const KEY: &[u8] = b"downward-signed-path-trust-root";
const CELL: &str = "cell-lon-1";
const OPENED: i64 = 1_760_000_000_000_000_000;

/// Large enough for a whole signed policy payload, so that when a peer frame
/// carrying one is refused it is refused for what it *is*, not for its size.
const MAX_FRAME: usize = 65_536;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_nanos(OPENED + ms * 1_000_000)
}

fn cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON"));
    Cell::new(
        config,
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
}

/// A peer endpoint on the same node, holding one live opportunity epoch.
fn live_peer_endpoint() -> PeerEndpoint {
    let leg = |id: &str, node: &str| EpochLeg {
        leg: id.into(),
        execution: None,
        node: node.into(),
        size: Decimal::from_int(100),
        unwind_loss: Decimal::from_int(2),
    };
    let ceiling = Duration::from_millis(500);
    let epoch = EpochDraft {
        opportunity: Some("opp-1".into()),
        sequence: Some(1),
        cycle: Some("usd>eur>usd".into()),
        legs: Some(vec![leg("a", "americas"), leg("b", "europe")]),
        size: Some(Decimal::from_int(100)),
        min_edge_bps: Some(Decimal::from_int(5)),
        model_versions: Some(vec!["m-7".into()]),
        opened_at_nanos: Some(OPENED),
        ttl: Some(Duration::from_millis(200)),
        max_recovery_loss: Some(Decimal::from_int(10)),
    }
    .build(ceiling)
    .unwrap();
    let mut endpoint = PeerEndpoint::new(
        "europe",
        PeerLimits {
            max_frame: MAX_FRAME,
            ttl_ceiling: ceiling,
            max_clock_offset: Duration::from_millis(5),
            max_fan_out: 2,
        },
    );
    endpoint.observe_clock_offset(0);
    let token = encode(&PeerMessage::Token { epoch }, MAX_FRAME).unwrap();
    endpoint.receive(&token, at(0)).unwrap();
    endpoint
}

/// A length-prefixed peer frame of `kind` carrying `payload` verbatim.
fn peer_frame(kind: &str, payload: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::json!({ "kind": kind, "payload": payload }).to_string();
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body.as_bytes());
    frame
}

/// Mutation (run 2026-10-04): strike `policy` and `policy_pack` from
/// `DOWNWARD_ONLY` in `qip_mesh::peer` — the centre's signed policy inside a
/// peer frame is then read as an unknown message instead of being refused as
/// the downward path's, and the "never from a peer" assertion fails. And:
/// make `VerifiedPolicy::verify` skip its MAC comparison — the unsigned
/// payload below then verifies with the peer epoch live.
#[test]
fn with_a_peer_epoch_live_on_the_node_a_policy_or_halt_reaches_the_cell_only_down_the_verified_path()
-> Result<()> {
    let mut endpoint = live_peer_endpoint();
    let mut cell = cell()?;
    assert_eq!(
        endpoint.current_sequence("opp-1"),
        Some(1),
        "premise: the peer channel is live on this node, holding an epoch"
    );
    assert!(
        cell.policy_sequence().is_none() && !cell.is_halted(),
        "premise: the cell has applied no policy and is not halted"
    );

    let policy = PolicyPayload::unproduced(7, CELL, at(1)).signed(KEY)?;
    let halt = HaltCommand::new(CELL, at(2), "centre decided").signed(KEY)?;
    assert!(
        VerifiedPolicy::verify(policy.clone(), KEY, CELL, at(3)).is_ok()
            && VerifiedHalt::verify(halt.clone(), KEY, CELL, at(3)).is_ok(),
        "premise: both are the genuine article and verify down the signed path"
    );

    // The centre's own signed bytes, handed to the node by a peer. A valid
    // signature must not make the peer channel a way in.
    let refused_before = endpoint.refusals().len();
    let signed_policy = serde_json::to_value(&policy).unwrap();
    for kind in ["policy", "policy_pack"] {
        let frame = peer_frame(kind, &signed_policy);
        assert!(
            frame.len() <= MAX_FRAME,
            "premise: the frame fits, so only its kind can refuse it"
        );
        let refusal = endpoint.receive(&frame, at(4)).unwrap_err().to_string();
        assert!(
            refusal.contains("never from a peer"),
            "a genuinely signed `{kind}` delivered by a peer was not refused as the downward \
             path's: {refusal}"
        );
    }
    // A halt has no peer message at all, so the same holds for it without a
    // rule naming it: the frame is not one the protocol can read.
    let halt_frame = peer_frame("halt", &serde_json::to_value(&halt).unwrap());
    assert!(
        endpoint.receive(&halt_frame, at(4)).is_err(),
        "a signed halt delivered by a peer was accepted by the peer endpoint"
    );
    assert_eq!(
        endpoint.refusals().len(),
        refused_before + 3,
        "each refusal is journaled"
    );
    assert_eq!(
        endpoint.current_sequence("opp-1"),
        Some(1),
        "a refused frame changed the peer endpoint's state"
    );

    // With the peer channel live, the downward path refuses exactly what it
    // refused before there was one.
    let unsigned = PolicyPayload::unproduced(8, CELL, at(1));
    let mut altered = policy.clone();
    altered.halted = true;
    let rekeyed = PolicyPayload::unproduced(8, CELL, at(1)).signed(b"a-peers-own-key")?;
    let foreign = PolicyPayload::unproduced(8, "cell-tok-1", at(1)).signed(KEY)?;
    for (what, payload) in [
        ("unsigned", unsigned),
        ("altered after signing", altered),
        ("signed by another key", rekeyed),
        ("signed for another cell", foreign),
    ] {
        assert!(
            VerifiedPolicy::verify(payload, KEY, CELL, at(5)).is_err(),
            "a policy {what} verified with a peer epoch live on the node"
        );
    }
    let forged_halt = HaltCommand::new(CELL, at(2), "a peer decided").signed(b"a-peers-own-key")?;
    assert!(
        VerifiedHalt::verify(forged_halt, KEY, CELL, at(5)).is_err(),
        "a halt signed by another key verified with a peer epoch live on the node"
    );
    assert!(
        cell.policy_sequence().is_none() && !cell.is_halted(),
        "the cell changed before anything verified reached it"
    );

    // And admits what it admitted: the gate is not merely shut.
    cell.apply_policy(VerifiedPolicy::verify(policy, KEY, CELL, at(6))?, at(6))?;
    assert_eq!(
        cell.policy_sequence(),
        Some(7),
        "the signed policy did not apply down the verified path"
    );
    cell.apply_halt(VerifiedHalt::verify(halt, KEY, CELL, at(7))?, at(7));
    assert!(cell.is_halted(), "the signed halt did not stop the cell");
    assert_eq!(
        endpoint.current_sequence("opp-1"),
        Some(1),
        "downward traffic moved the peer endpoint"
    );
    Ok(())
}

/// The text of `pub struct <name> { … }` and the derive line above it.
fn declaration(source: &str, name: &str) -> (String, String) {
    let opening = format!("pub struct {name} {{");
    let at = source
        .find(&opening)
        .unwrap_or_else(|| panic!("`{opening}` is not declared where this test reads"));
    let derive = source[..at]
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("#[derive("))
        .unwrap_or_else(|| panic!("{name} has no derive line above it"))
        .to_string();
    let body = source[at + opening.len()..]
        .split_once("\n}")
        .map(|(body, _)| body.to_string())
        .unwrap_or_default();
    (derive, body)
}

/// Mutation (run 2026-10-04): add `serde::Deserialize` to `VerifiedPolicy`'s
/// derive — the first assertion fails; a peer frame, or any other bytes,
/// could then be parsed straight into a "verified" policy. And: add a second
/// constructor, `pub fn trusted(inner: PolicyPayload, verified_at: Timestamp)
/// -> Self { Self { inner, verified_at } }` — the construction count fails.
/// And: make `VerifiedHalt`'s `inner` field `pub` — the privacy check fails.
#[test]
fn nothing_but_the_cells_own_verification_can_mint_a_verified_policy_halt_or_grant_and_the_peer_protocol_cannot_name_one()
 {
    // "There is no other way to obtain one" is a doc comment on each of the
    // three types. It is true because their fields are private and nothing
    // derives a constructor for them — and it stops being true, silently,
    // the day somebody adds `Deserialize` so a frame can be decoded straight
    // into one, which is exactly what wiring a new inbound channel invites.
    for (path, names) in [
        (
            "backend/crates/edge/qip-edge/src/policy.rs",
            vec!["VerifiedPolicy", "VerifiedHalt"],
        ),
        (
            "backend/crates/edge/qip-edge/src/envelope.rs",
            vec!["VerifiedEnvelope"],
        ),
    ] {
        let source = read(path);
        for name in &names {
            let (derive, body) = declaration(&source, name);
            for forbidden in ["Deserialize", "Default"] {
                assert!(
                    !derive.contains(forbidden),
                    "{path}: {name} derives {forbidden} ({derive}); a value nobody verified \
                     can then be made without calling `verify`"
                );
            }
            let fields: Vec<&str> = body
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with("//"))
                .collect();
            assert!(
                !fields.is_empty(),
                "premise: {name}'s fields were read; an empty body would make the privacy \
                 check below pass on nothing"
            );
            for field in fields {
                assert!(
                    !field.starts_with("pub"),
                    "{path}: {name} exposes `{field}`; a public field is a struct literal \
                     any crate can write"
                );
            }
            for sidestep in [
                format!("for {name}"),
                format!("-> {name}"),
                format!("{name} {{ inner"),
            ] {
                // `impl … for VerifiedX` would be a derived or hand-written
                // conversion; `-> VerifiedX` a free constructor.
                let offenders: Vec<&str> = source
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .filter(|line| line.contains(&sidestep))
                    .collect();
                assert!(
                    offenders.is_empty(),
                    "{path}: {name} can be obtained without `verify`: {offenders:?}"
                );
            }
        }

        // One construction per `verify`, and nowhere else in the module.
        let code: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        //
        // ADR 0110 gave each type a second door, `verify_with`, which checks
        // the signature against the cell's `TrustRoot` (the P-256 bundle, or
        // the shared key for its last release) rather than a bare key. That
        // is a verification, so it is counted as one — but only after the
        // body is shown to ask the root before it constructs, because a
        // `verify_with` that skipped the check would otherwise be admitted
        // by its name alone.
        let verifies = code.matches("pub fn verify(").count();
        let root_verifies = code.matches("pub fn verify_with(").count();
        let constructions = code.matches("Self {").count();
        assert_eq!(
            verifies,
            names.len(),
            "premise: {path} declares one `verify` per verified type"
        );
        assert_eq!(
            root_verifies,
            names.len(),
            "premise: {path} declares one ADR 0110 `verify_with` per verified type"
        );
        for (index, _) in code.match_indices("pub fn verify_with(") {
            let body = &code[index..];
            let construct = body.find("Ok(Self {").unwrap_or_else(|| {
                panic!("{path}: a `verify_with` constructs nothing the scan can find")
            });
            let before = &body[..construct];
            assert!(
                before.contains("root.check(") && before.contains("ArtefactClass::"),
                "{path}: a `verify_with` constructs a verified value without first asking the \
                 trust root to check its signature under a named artefact class"
            );
        }
        assert_eq!(
            constructions,
            verifies + root_verifies,
            "{path} constructs a verified value {constructions} times for {} `verify` and \
             `verify_with` functions; the extra one is a way in that checks no signature",
            verifies + root_verifies
        );
    }

    // The activation seam takes the verified types and nothing looser.
    let cell = read("backend/crates/edge/qip-edge/src/cell.rs");
    for signature in [
        "pub fn apply_policy(&mut self, verified: VerifiedPolicy,",
        "pub fn apply_halt(&mut self, halt: VerifiedHalt,",
    ] {
        assert!(
            cell.contains(signature),
            "Cell no longer declares `{signature}`; if it takes a bare payload now, every \
             caller is trusted to have verified it"
        );
    }

    // And the peer protocol cannot reach that seam at all: it does not link
    // the cell, so there is no type it could hand across.
    let manifest = read("backend/crates/services/qip-mesh/Cargo.toml");
    assert!(
        manifest.contains("[dependencies]"),
        "premise: the mesh crate's manifest was read"
    );
    assert!(
        !manifest
            .lines()
            .any(|line| line.trim_start().starts_with("qip-edge")),
        "qip-mesh depends on the edge cell; the peer protocol can now construct or apply \
         what only the signed downward path may"
    );
    // The protocol lives in `qip-peer` since ADR 0116 (it was in
    // `qip-contracts` from W5.4 until then), and its link in `qip-peer-link`;
    // neither links a cell, and the same property holds of both.
    for crate_path in ["libs/qip-peer", "edge/qip-peer-link"] {
        let manifest = read(&format!("backend/crates/{crate_path}/Cargo.toml"));
        assert!(
            manifest.contains("[dependencies]")
                && !manifest
                    .lines()
                    .any(|line| line.trim_start().starts_with("qip-edge")),
            "{crate_path} depends on the edge cell; the peer protocol can now construct or \
             apply what only the signed downward path may"
        );
    }
    let peer = read("backend/crates/libs/qip-peer/src/lib.rs");
    assert!(
        peer.contains("DOWNWARD_ONLY"),
        "premise: this is the module that refuses downward kinds from a peer"
    );
    // Code only, and `qip_edge` as a whole path segment. Round 2's move
    // gave the module a doc line naming the transport that carried it,
    // `qip_edge_node::peer`, and a raw `contains("qip_edge")` is true of
    // `qip_edge_node` — the substring trap the testing rules name. The crate
    // the property is about is `qip_edge`, the cell; a lib cannot name the
    // app either way. The other three stay substring matches on purpose:
    // `Verified` is meant to catch every `Verified*` type.
    let code: String = peer
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("DOWNWARD_ONLY"),
        "premise: the comment filter left the module's code in place"
    );
    let names_the_cell_crate = code.match_indices("qip_edge").any(|(at, _)| {
        let after = code[at + "qip_edge".len()..].chars().next();
        let before = code[..at].chars().next_back();
        !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            && !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    assert!(
        !names_the_cell_crate,
        "the peer protocol names `qip_edge`; a peer frame must never be on a path to the \
         cell's activation seam"
    );
    for name in ["apply_policy", "apply_halt", "Verified"] {
        assert!(
            !code.contains(name),
            "the peer protocol names `{name}`; a peer frame must never be on a path to the \
             cell's activation seam"
        );
    }
}

/// A payload whose every slot is produced, as the centre signs it when it
/// has something for each: the trained model manifest, the compiled plan,
/// belief priors, the episodic and causal digests, the regime, capital
/// grants, the cycle whitelist, the risk envelope, inventory targets,
/// feasibility constraints, adversary profiles, dispositions, opportunity
/// definitions and the acceptable builds. Contents are small but not empty
/// where emptiness would be the same bytes as a tampered value.
fn every_slot_produced(sequence: u64, issued: Timestamp) -> PolicyPayload {
    use qip_contracts::policy::{
        AdversaryProfiles, BeliefPriors, BuildSet, CausalDigest, CycleWhitelist, Dispositions,
        EpisodicDigest, FeasibilityConstraints, GrantManifest, InventoryTargets, ModelManifest,
        OpportunityManifest, PlanDigest, RegimeState, RiskEnvelopeSnapshot, Slot,
    };
    use std::collections::{BTreeMap, BTreeSet};
    let map = |k: &str, v: &str| BTreeMap::from([(k.to_string(), v.to_string())]);
    let dec = |k: &str, v: i64| BTreeMap::from([(k.to_string(), Decimal::from_int(v))]);
    let mut p = PolicyPayload::unproduced(sequence, CELL, issued);
    p.trained_models = Slot::produced(
        ModelManifest::unreleased(map("fill-probability", "sha256:aa")),
        issued,
    );
    p.compiled_plan = Slot::produced(
        PlanDigest {
            digest: "plan-1".into(),
            strategies: 2,
        },
        issued,
    );
    p.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::from([("rates".to_string(), 0.25)]),
        },
        issued,
    );
    p.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "ep-1".into(),
            episodes: 3,
        },
        issued,
    );
    p.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: vec!["rates->fx".into()],
        },
        issued,
    );
    p.regime_state = Slot::produced(
        RegimeState {
            regime: "trend/calm".into(),
            confidence: 0.6,
        },
        issued,
    );
    p.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec!["grant-1".into()],
        },
        issued,
    );
    p.cycle_whitelist = Slot::produced(
        CycleWhitelist {
            cycles: map("usd>eur>usd", "digest-1"),
            conversions: Vec::new(),
            start_sizes: BTreeMap::new(),
        },
        issued,
    );
    p.risk_envelope = Slot::produced(
        RiskEnvelopeSnapshot {
            limits: serde_json::json!({ "max_gross_notional": "1000000" }),
        },
        issued,
    );
    p.inventory_targets = Slot::produced(
        InventoryTargets {
            targets: dec("XLON:ACME", 100),
            reference_prices: dec("XLON:ACME", 50),
        },
        issued,
    );
    p.feasibility_constraints = Slot::produced(
        FeasibilityConstraints {
            minimum_order: dec("XLON", 1),
            fee_floor: dec("XLON", 0),
            tick: dec("XLON", 1),
            withdrawn_venues: BTreeSet::new(),
            dark_regions: BTreeSet::new(),
        },
        issued,
    );
    p.adversary_profiles = Slot::produced(
        AdversaryProfiles {
            venues: BTreeMap::from([("XLON".to_string(), serde_json::json!({"posture": "calm"}))]),
        },
        issued,
    );
    // A produced but empty disposition set is off the wire by design (it
    // says nothing), so one unwind makes the slot a signed one.
    p.dispositions = Slot::produced(
        Dispositions {
            unwinds: BTreeMap::from([(
                qip_contracts::signal::StrategyId::new("mean-reversion"),
                dec("XLON:ACME", 10),
            )]),
        },
        issued,
    );
    p.opportunity_definitions = Slot::produced(
        OpportunityManifest {
            opportunities: map("opp-1", "definition-1"),
        },
        issued,
    );
    p.acceptable_builds = Slot::produced(
        BuildSet {
            builds: BTreeSet::from(["build-1".to_string()]),
        },
        issued,
    );
    p
}

/// Every slot name [`every_slot_produced`] fills, in declaration order.
const SLOTS: [&str; 15] = [
    "trained_models",
    "compiled_plan",
    "belief_priors",
    "episodic_digest",
    "causal_digest",
    "regime_state",
    "capital_grants",
    "cycle_whitelist",
    "risk_envelope",
    "inventory_targets",
    "feasibility_constraints",
    "adversary_profiles",
    "dispositions",
    "opportunity_definitions",
    "acceptable_builds",
];

/// `slot` of `p`, re-stamped at `later` (same value) when `strip` is false,
/// and unproduced when it is true. Both are edits a relay could make without
/// the key: a slot moved forward in time reads fresher than the centre said,
/// and a slot dropped narrows nothing the cell can tell was signed.
fn tamper(p: &mut PolicyPayload, slot: &str, strip: bool, later: Timestamp) -> bool {
    use qip_contracts::policy::Slot;
    macro_rules! edit {
        ($field:ident) => {{
            let value = p.$field.value().cloned();
            p.$field = match (strip, value) {
                (true, _) => Slot::unproduced(),
                (false, Some(v)) => Slot::produced(v, later),
                (false, None) => return false,
            };
            true
        }};
    }
    match slot {
        "trained_models" => edit!(trained_models),
        "compiled_plan" => edit!(compiled_plan),
        "belief_priors" => edit!(belief_priors),
        "episodic_digest" => edit!(episodic_digest),
        "causal_digest" => edit!(causal_digest),
        "regime_state" => edit!(regime_state),
        "capital_grants" => edit!(capital_grants),
        "cycle_whitelist" => edit!(cycle_whitelist),
        "risk_envelope" => edit!(risk_envelope),
        "inventory_targets" => edit!(inventory_targets),
        "feasibility_constraints" => edit!(feasibility_constraints),
        "adversary_profiles" => edit!(adversary_profiles),
        "dispositions" => edit!(dispositions),
        "opportunity_definitions" => edit!(opportunity_definitions),
        "acceptable_builds" => edit!(acceptable_builds),
        _ => false,
    }
}

/// R12-CONTRACT-1195: a cell installs only packs whose signature verifies,
/// and a tampered pack is refused — for every pack the centre sends down,
/// not only the halt flag the test above alters. The model manifest, the
/// strategy plan, the belief priors, the capital grants and the risk
/// envelope are each a slot of the one signed `PolicyPayload`, so "a
/// tampered pack" is a payload with one slot edited after signing. Each of
/// the fifteen slots, re-stamped later or stripped, is refused by
/// `VerifiedPolicy::verify` — the only way to a `VerifiedPolicy`, which is
/// the only argument `Cell::apply_policy` takes — and leaves the cell where
/// it was; the untampered payload, with every slot produced, then applies.
///
/// The failure this prevents: a slot outside the signing string. A slot
/// added to the struct and forgotten in `signing_payload` would be
/// installable by anyone between the centre and the cell, and the halt-only
/// tamper above would never notice. Mutation: make `signing_payload` omit
/// `risk_envelope` — re-stamping or stripping that slot then verifies and
/// the assertion naming `risk_envelope` fails.
#[test]
fn each_signed_pack_tampered_after_signing_is_refused_and_the_untampered_payload_installs()
-> Result<()> {
    let issued = at(1);
    let later = at(2);
    let signed = every_slot_produced(9, issued).signed(KEY)?;
    let mut cell = cell()?;
    assert!(
        cell.policy_sequence().is_none(),
        "premise: the cell has applied no policy"
    );
    assert!(
        VerifiedPolicy::verify(signed.clone(), KEY, CELL, at(3)).is_ok(),
        "premise: the untampered payload verifies"
    );
    for slot in SLOTS {
        for strip in [false, true] {
            let mut tampered = signed.clone();
            assert!(
                tamper(&mut tampered, slot, strip, later),
                "premise: `{slot}` was produced in the signed payload"
            );
            assert_ne!(
                serde_json::to_value(&tampered).unwrap(),
                serde_json::to_value(&signed).unwrap(),
                "premise: tampering `{slot}` changed the payload"
            );
            let how = if strip { "stripped" } else { "re-stamped" };
            let refused = VerifiedPolicy::verify(tampered, KEY, CELL, at(3));
            assert!(
                refused.is_err(),
                "a payload with `{slot}` {how} after signing verified, so a cell would install it"
            );
            assert!(
                refused
                    .unwrap_err()
                    .to_string()
                    .contains("does not verify against this cell's key"),
                "`{slot}` {how} was refused for something other than its signature"
            );
        }
    }
    assert!(
        cell.policy_sequence().is_none(),
        "the cell changed before anything verified reached it"
    );
    cell.apply_policy(VerifiedPolicy::verify(signed, KEY, CELL, at(3))?, at(3))?;
    assert_eq!(
        cell.policy_sequence(),
        Some(9),
        "the untampered payload did not install"
    );
    Ok(())
}
