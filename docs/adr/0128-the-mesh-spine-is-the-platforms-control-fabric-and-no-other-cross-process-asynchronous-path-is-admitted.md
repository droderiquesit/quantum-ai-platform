# ADR 0128: The mesh spine is the platform's control fabric, and no other cross-process asynchronous path is admitted

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It names what exists, moves no
  traffic and admits nothing.
- **Date**: 2026-10-18
- **Extends**: ADR 0011 (the network mesh replaced the managed bus), ADR 0100
  (the event fabric), ADR 0116 (the Lane 1 peer link), ADR 0110 (signed
  artefacts).
- **Keeps**: ADR 0008 (cells decide alone), ADR 0111 (P0/P1 durability is
  Spanner-backed), ADR 0119 (the mirror is a fabric consumer).

## Context

Three register rows ask that control propagation use "the Algorik Rust Event
& Control Fabric": R12-FABRIC-1027 (control propagation), R12-FABRIC-10077
(publish policy, research, capital and risk-envelope events to the regional
Fabric) and R12-FABRIC-10177 (all internal asynchronous contracts use the
Fabric SDK and schema model). Each is PARTIAL with the same finding: capital
envelopes and policy frames go down the `qip-mesh` spine over HTTP/1.1 from
`qip-api/src/mesh.rs`, and are verified at `qip-edge/src/mesh.rs`, not
produced to a fabric P0 stream; only the edge's outcomes reach the fabric
(through `FabricSink`) and `qip-ledgerd` consumes them.

What the spine already guarantees, by symbol:

- **Down is typed, and durable where order matters.**
  `qip_mesh::spine::CapitalDispatcher` persists each `CapitalGrantFrame` to
  a spool before sending, so a crash between grant and send redelivers
  rather than loses it. `PolicyCourier` carries `PolicyFrame` and
  `HaltFrame` **deliberately unspooled**: its doc says a policy payload is
  state (the newest wins and a missed one is superseded by the next
  publish) and a halt is idempotent and republished with every later
  payload's flag, so spooling either "would preserve exactly the staleness
  the payload's TTLs exist to expire".
- **Down is signed and replay-proof per cell.** Envelopes and policy
  payloads are signed under ADR 0110's classes; `PolicyPayload.sequence` is
  "strictly increasing per cell ... a cell refuses any sequence at or below
  the last it applied", and a payload for another cell is refused however
  genuine its signature.
- **Up is idempotent state, not a log.** Cell deltas are not durable by
  design (`qip-mesh/src/spine.rs` module doc: "Up: state deltas, not durable,
  not decoded here"); the centre refuses a redelivery it has absorbed and
  hands each frame to the composition root's sink, which feeds
  `Platform::ingest_cell_report`.

Locate them with
`grep -n 'pub struct CapitalDispatcher\|pub struct PolicyCourier\|pub struct PolicyFrame\|pub struct HaltFrame\|pub struct CellDeltaReceiver' backend/crates/services/qip-mesh/src/spine.rs`.

Moving these frames onto a fabric P0 stream would buy a second durable copy
of facts the spool already holds durably, would put Spanner (ADR 0111) on
the path a cell's halt travels, and would let a cell's capital depend on a
regional broker being reachable — the coupling ADR 0008 exists to refuse.

## Decision

1. **The control fabric has two transports, each with one job.** The **mesh
   spine** carries *signed downward control* (capital envelopes, policy
   payloads, halt commands) and *upward cell state* (deltas). The **event
   fabric** carries *records*: outcomes, chain spans, ledger operations and
   the P0–P4 streams ADR 0100 catalogues. The Lane 1 **peer link** (ADR
   0116) carries cell-to-cell arbitrage traffic. Those three are the
   platform's cross-process asynchronous paths; request–response HTTP
   (the API, the health and metrics endpoints) is synchronous and is not one.
2. **The spine is the "Rust Event & Control Fabric"'s control half** for the
   purpose of R12-FABRIC-1027, 10077 and 10177: it is Rust, in-tree, typed
   end to end, signed, replay-refusing per cell, durable for the ordered
   instruction (capital) and superseding for state (policy, halt). The
   rows' requirement is met by naming, not by moving traffic, and
   the register should be re-read on that basis.
3. **Research events stay on the fabric**, not the spine: they are records,
   not control, and nothing in a cell acts on them. If research output must
   reach a cell, it reaches it as a signed artefact on the policy payload
   (a model pack, a reflex package), which is control and so travels the
   spine.
4. **No fourth path.** An acceptance test in `qip-acceptance` scans every
   app crate's sources and refuses a cross-process send that is not one of:
   a `qip_mesh::spine` frame, a `qip_events` fabric producer, a
   `qip_peer_link` session, or a request through `qip_transport`'s HTTP client
   to a named synchronous route. The test enumerates the allowed seams by
   symbol so a new transport fails it by name, and it is mutation-verified by
   adding a raw socket write to a scratch module.
5. **Each spine frame is catalogued beside the fabric's streams.** The spine's
   frame types, their direction, their signing class and their durability
   are listed in the same catalogue document the fabric's streams are, so
   "every internal asynchronous contract" has one place a person reads.

## Consequences

- No traffic moves; the rows' remaining work is the scan (decision 4) and the
  catalogue entry (decision 5).
- A future proposal to put control on the fabric has to argue against
  decision 2 and ADR 0008, rather than being the default reading of the
  requirement.
- Code slices left: the transport scan test (R12-FABRIC-10177) and the
  catalogue rows for `CapitalGrantFrame`, `PolicyFrame`, `HaltFrame` and the
  delta frame (R12-FABRIC-1027, 10077).

## What it costs

- Two transports to reason about instead of one. An operator debugging "why
  did the cell not get its grant" reads the spine's spool and dispatcher,
  not a fabric consumer lag; the observability for each is separate.
- The spine's upward path is not durable, and neither is policy on the way
  down; both are recovered by the next frame, by design. A delta lost in
  flight is
  recovered only by the next delta, so the centre's picture of a cell can lag
  by one delta interval; that is the design today and this record keeps it.
- The blueprint's phrase "all internal asynchronous contracts use the Fabric
  SDK" is satisfied by a reading, and a reviewer who holds the literal
  reading will disagree with the register.

## What would make this wrong

- If a consumer other than the cell needed the downward control stream (an
  auditor, a second region's mirror, a replay tool), the spine's point-to-point
  spool cannot serve it, and that consumer's need is the argument for also
  producing control frames to a fabric stream — as a record, never as the
  path the cell acts on.
- If the capital spool and the fabric's durability ever diverged in what they
  promise (for example a spool that can drop on disk pressure), or if a
  policy payload came to carry an ordered instruction rather than state, so
  that a missed one is not superseded by the next, decision 2's equivalence
  would be false and the rows would be PARTIAL on substance.

## Rows

R12-FABRIC-1027, R12-FABRIC-10077, R12-FABRIC-10177. Each stays PARTIAL until
the scan and catalogue land with a mutated test.
