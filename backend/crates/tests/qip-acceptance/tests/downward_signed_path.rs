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
        let verifies = code.matches("pub fn verify(").count();
        let constructions = code.matches("Self {").count();
        assert_eq!(
            verifies,
            names.len(),
            "premise: {path} declares one `verify` per verified type"
        );
        assert_eq!(
            constructions, verifies,
            "{path} constructs a verified value {constructions} times for {verifies} `verify` \
             functions; the extra one is a way in that checks no signature"
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
    let peer = read("backend/crates/services/qip-mesh/src/peer.rs");
    assert!(
        peer.contains("DOWNWARD_ONLY"),
        "premise: this is the module that refuses downward kinds from a peer"
    );
    for name in ["qip_edge", "apply_policy", "apply_halt", "Verified"] {
        assert!(
            !peer.contains(name),
            "the peer protocol names `{name}`; a peer frame must never be on a path to the \
             cell's activation seam"
        );
    }
}
