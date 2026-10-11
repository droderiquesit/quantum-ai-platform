# ADR 0116: Lane 0 is the reflex pass, and the edge node hosts the peer link across a non-blocking port

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation, under the instruction to adopt the recommended answer for
  every open decision: "Update as needed remove my decisions make the
  recommended".
- **Date**: 2026-10-11
- **Requirements**: MESH-013 (no Lane 0 code path can await a coordination
  function), MESH-002 (a partitioned region decides alone), MESH-5050 (a peer
  outage holds neither the journal flush nor the policy downlink), ARCH-013,
  DATA-058 (no fast-lane crate can wait on a warehouse), blueprint §4 (the five
  Lane 1 functions) and §16 (peer coordination is node-to-node, never
  broker-mediated).
- **Amends**: `docs/architecture/lane-placement.md`'s definition of Lane 0, and
  the "slowest lane it hosts" rule as it applies to `qip-edge-node`.
- **Implemented by**: not yet. The design below is handed to an implementer.
  Until that lands, `mesh_lane.rs::the_five_coordination_functions_are_lane_one_and_no_lane_zero_crate_can_name_one_to_await_it`
  keeps failing, and it is right to.

## Context

Commit `215138c2` gave the peer protocol a wire. It moved the protocol from
Lane 1 `qip-mesh` into `qip-contracts::peer`, so that the edge node could link
it without `qip-mesh`'s BigQuery closure (DATA-058). `qip_mesh::peer` became a
re-export. It also added `qip-edge-node/src/peer.rs`: a thread that owns the
peer listener and connections, and that the scheduler meets at two bounded
`sync_channel`s through `try_send` and `try_recv` after the flush.

The non-blocking wire is sound engineering. The register's model of it is
broken, for two reasons:

1. **The register cannot express a binary that hosts a peer link.**
   `lane-placement.md` defines Lane 0 as the normal-dependency closure of
   `qip-edge-node`, and says a crate that composes slower crates is placed in
   the slowest lane it hosts. Blueprint §16 requires the node to host the peer
   link, because the hop must be node-to-node. Under those two rules
   the node would be Lane 1, its closure would stop defining Lane 0, and so
   Lane 0 would have no definition at all.
2. **The protocol is now in a Lane 0 crate, and parts of it run on the
   decision thread.** `qip-contracts` is in every reflex crate's closure.
   `PeerSession::turn` runs `PeerEndpoint::receive`, `edge_state_frame` and
   `forget_expired` on the scheduler's thread, and it takes `&Cell`. The
   queues are non-blocking, but the endpoint's work is not: it is
   coordination done inside the decision loop. MESH-013 fails on this, and the
   failure is correct.

## Decision

1. **The protocol leaves Lane 0.** `qip-contracts/src/peer.rs` moves, with
   `git mv` so its history follows, to a new library crate
   **`backend/crates/libs/qip-peer`**. It is registered in Lane 1. Its
   normal dependencies are `serde`, `serde_json` and `qip-core`, and nothing
   else, so DATA-058 still holds. `qip-mesh` replaces `pub use qip_contracts::peer;`
   with `pub use qip_peer as peer;`, so every central path that names
   `qip_mesh::peer::…` keeps compiling unchanged. `qip-peer` opens no socket,
   so `only_the_named_libraries_open_sockets` still needs no third exception.
2. **The wire leaves the node, and the endpoint leaves the decision thread.**
   A new crate, **`backend/crates/edge/qip-peer-link`**, is registered in
   Lane 1 and depends on `qip-peer` and `qip-core`. It holds what
   `qip-edge-node/src/peer.rs` holds today: `PeerSettings`, `PeerLink`, the
   wire thread, `node_limits` and `PeerSession`. `PeerSession::turn` takes
   `Vec<ExecutableEdge>` instead of `&Cell`. It sits under `edge/`, not
   `libs/`, because it owns `TcpListener` and `TcpStream`, and a library that
   opens a socket is a service in the wrong directory (`00-boundaries.md`). In
   production the wire thread owns the `PeerSession`, and so the endpoint as
   well: receiving, refusing, framing edge state and expiring epochs all run
   on that thread and never on the scheduler's.
3. **The decision loop meets Lane 1 through one named type.** `qip-peer-link`
   exports `boundary::ReflexPort` and a plain-data `boundary::EdgeQuote`, which
   carries venue, instrument, bid, bid size, ask, ask size and `as_of_ns`. The
   port has exactly these methods, and none of them can block:
   - `try_publish(Vec<EdgeQuote>) -> bool`: a `try_send` into a bounded queue.
     A full queue is a counted drop.
   - `status(&self) -> PeerStatus`: a copy of atomics the wire thread writes
     (frames received and refused, edge peers held, refusals, send failures,
     dropped offers). No lock is taken.

   The port has no `recv`, `recv_timeout`, `join`, `lock`, `wait` or `sleep`
   method, and no method that returns a coordination type. The wire thread
   converts `EdgeQuote` into `ExecutableEdge`, and validates it, on its own
   side. Nothing in the reflex path can wait for a peer because nothing there
   holds a handle that could wait.
4. **Lane 0 is redefined as the reflex pass's closure.** The register's
   definition becomes:
   - **Lane 0** is the normal-dependency closure of `qip-edge-node`, minus the
     node itself and the Lane 1 crates it hosts through the reflex port. That
     hosted set is exactly `qip-peer-link` and `qip-peer`, and the test fixes
     it as a constant. It always contains `qip-edge` and its whole closure.
   - **`qip-edge-node`** is registered at **Lane 1** under the existing
     "slowest lane it hosts" rule. Its requirement reads: *the cell's
     composition root. Hosts Lane 0 (the cell) and Lane 1 (the peer link), and
     its decision loop reaches Lane 1 only through `qip_peer_link::boundary`.*
5. **MESH-013 keeps its teeth and gains the node's.** The test must fail if:
   - any crate in `closure(qip-edge)`, or any crate registered at Lane 0,
     depends on `qip-mesh`, `qip-peer` or `qip-peer-link`, or names
     `qip_mesh`, `qip_peer`, `qip_peer_link` or one of the five coordination
     types in non-comment source. This is the old scan over the narrower,
     correct set. `qip-contracts` stays in that set and must now come up
     clean;
   - any crate other than the fixed hosted set appears at Lane 1 or slower in
     `closure(qip-edge-node)`;
   - any `qip-edge-node/src` file other than `main.rs`, the composition root
     that spawns the link, names `qip_peer` or `qip_peer_link` except as
     `qip_peer_link::boundary::…`, or names a coordination type, `PeerSession`
     or `PeerLink`;
   - `qip-peer-link/src/boundary.rs` declares a `pub fn` outside the permitted
     list (`try_publish`, `status`, plus the constructor `pub(crate)`), or
     contains `.recv(`, `recv_timeout`, `.join(`, `.lock(`, `.wait(` or
     `sleep`.

   It keeps its premises. The registry says five functions, all in Lane 1.
   The host lane must equal the `qip-mesh` row, and the register must also
   place `qip-peer` in Lane 1. The definitions are found in
   `backend/crates/libs/qip-peer/src/lib.rs`, and the kernel scan still finds
   `qip_mesh`.

## Dependency direction

```
qip-core, serde, serde_json   (libs, Lane 0)
        ↑
qip-peer                      (libs, Lane 1; pure, no socket)
        ↑                  ↑
qip-peer-link (edge, L1)   qip-mesh (services, L1; re-exports qip_peer as peer)
        ↑
qip-edge-node (apps, L1 host) ──→ qip-edge (edge, L0) ──→ … Lane 0 closure
```

Every new edge points inward. A library depends only on libraries. The edge
crate `qip-peer-link` depends on libraries. The service `qip-mesh` depends on
a library. The app depends on edge crates. No library depends on a service, no
service depends on the runtime, and nothing depends on an app. `qip-edge` gains
no edge, and the test refuses any that would point at the peer crates. No
`tokio`, and no async at all. No third-party crate: the dependency allow-list
is unchanged.

## Rejected alternatives

- **Keep the protocol in `qip-contracts` and add an exemption to MESH-013.**
  Rejected because the exemption would be the whole guarantee. A Lane 0 crate
  that defines `PeerEndpoint` lets `Cell::work` construct one and await a
  reservation. The only thing preventing it would be a list of what the test
  ignores.
- **Leave the protocol in `qip-mesh` and have the node depend on it.**
  Rejected because that is how `215138c2` began. `qip-mesh` carries the
  BigQuery adapter (DATA-058), and the node would link a warehouse client.
- **Run the peer link as a separate process (a sidecar) beside the node.**
  Rejected for now. It satisfies MESH-013 with no in-binary rule. But it adds
  a deployable, a local IPC hop and a second health surface on a host that
  has never been applied (`execution_nodes = {}`). The port gives the same
  structural property with fewer moving parts. See the last section for when
  this flips.
- **Only move the protocol, and keep `PeerSession::turn` on the decision
  thread.** Rejected because the endpoint's receive, frame and expire work
  would still run inside the loop whose budget is the reflex pass's. That
  work is bounded, but it is coordination, and Lane 1 exists so that the
  reflex budget never pays for it.

## What it costs

- Two new workspace crates, two register rows, and a register definition
  stated as a difference: Lane 0 is the closure minus a named hosted set. That
  is less elegant than "the closure", and the hosted set is a constant that a
  reviewer must refuse to grow casually.
- The node's health body reads peer state from atomics, one frame behind the
  wire thread. Today it reads the endpoint directly. The figures are already
  diagnostic, and nothing decides on them.
- `qip-edge-node/tests/peer_mesh.rs` drives `endpoint_mut()` on two sessions.
  It moves to `qip-peer-link/tests/`, where `PeerSession` stays public for
  Lane 1 callers and tests. The node keeps one test proving that a pass with a
  peer configured publishes through the port and never stalls when the peer
  is absent.
- The `downward_signed_path` suite reads `qip-contracts/src/peer.rs` by path.
  It is repointed to `qip-peer/src/lib.rs`.

## What would make this wrong

- **A reflex decision that needs a peer's answer inside the pass.** One
  example is a multi-leg fire that must be confirmed before a leg is sent. The
  port deliberately cannot carry that. The answer must arrive as a decision
  the cell reads from its own state on a later pass, or the requirement goes
  to an ADR that argues it against MESH-002. A blocking method on the port is
  not the answer.
- **The hosted set growing.** If a second Lane 1 crate has to be hosted, the
  node is becoming a coordinator, and the sidecar alternative should be
  reopened rather than the constant widened.
- **The wire thread measurably contending with the pass.** Contention on a
  core or on the allocator, measured on the execution node's machine type,
  would be the evidence. That is the trigger to move the link into its own
  process.
- **ADR 0112's mTLS transport arriving.** If `rustls` cannot be confined to
  `qip-peer-link`, this placement needs revisiting. It should only replace
  the stream type there.
