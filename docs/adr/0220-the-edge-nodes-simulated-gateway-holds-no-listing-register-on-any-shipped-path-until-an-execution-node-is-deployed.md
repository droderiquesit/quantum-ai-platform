# ADR 0220: The edge node's simulated gateway holds no listing register on any shipped path until an execution node is deployed

- **Status**: **Accepted**, 2026-10-24, on delegated authority. The wave580
  parent session relayed on 2026-10-24 that the owner has delegated every
  remaining decision ("Make all decisions for me, I want the end result to
  have a full system", recorded by the parent in
  `docs/blueprint/owner-decisions-2026-10-24.md`), and assigned this item's
  choice — wire an opt-in register, or record the fail-closed default — to
  lane L7-5107. That record was not on any branch this lane could read when
  this was written; the acceptance rests on the parent's relay of it, and an
  owner who reads the delegation differently may reverse this record by the
  route under "What would make this wrong". It decides no instrument's
  permissions and no deployment.
- **Date**: 2026-10-24
- **Requirement**: R12-COVERAGE-5107 — "Created markets/instruments are
  supported only where venue, product and legal governance support them."
- **Keeps**: ADR 0003 (paper trading by default), ADR 0021 (markets are
  created on the simulated venue only), ADR 0024 (an execution node per
  region, never yet applied), ADR 0109 (no dependency is added).

## Context

`qip-edge-node`'s `SimulatedGateway` lists an instrument on demand — the
first placement, seeded touch or tape line naming it brings it into
existence at the simulated venue — and since 2026-10-24 it does so only
under a creation register it was handed: `create` and `list` from the venue,
the product rules and the jurisdiction for an instrument it invents, `list`
again at every placement and every release of a held order, and `seed` for
every seeded touch. `SimulatedGateway::new`, the constructor `main.rs`
reaches through `NodeGateway::open`, gives it no register at all, and with
none every listing is refused as `denied`, naming the missing permissions.

The round-4 skeptic on R12-COVERAGE-5107 (on `c266eb7a`) recorded the
consequence as its seventh item: no shipped path hands the node a register,
so the admitting half of the gate — the half that lets a permitted
instrument list — is proven only by tests that call
`SimulatedGateway::with_listing_permissions` themselves. The node as
`main.rs` composes it lists nothing, places nothing that needs a listing,
and seeds nothing.

Two answers were open: wire an opt-in register (a file named by a variable
read in `main.rs`, parsed into a `CreationPermissions`, refusing an
unreadable or unparseable file at start-up, with the empty register still
the default), or record that the fail-closed default is deliberate.

## Decision

**The fail-closed default stands, and no listing register is wired into
`qip-edge-node` until an execution node is deployed.**

1. `SimulatedGateway::new` keeps building a gateway with no register, and
   `NodeGateway::open` keeps calling it. No variable, file or flag is read
   for a register, so no environment can relax the default by
   configuration.
2. The admitting half stays proven by tests that construct the register
   explicitly
   (`qip-edge-node/tests/gateway.rs` and the edge-node and acceptance
   suites that list under `with_listing_permissions` or `permit_listings`).
   Those tests prove the gate admits a permitted instrument and refuses an
   unpermitted one; they do not prove that any deployed process lists
   anything, and nothing in this repository should say that one does.
3. When a node is to be deployed, the register is wired in the same change
   that deploys it, under the shape described below, and this record is
   superseded by the ADR that authorises the deployment.

## Why not wire it now

- **Nothing would read it.** `execution_nodes = {}` in test, stage and prod,
  and dev declares `newyork-1` with `execution_nodes_enabled = false`
  (`grep -n 'execution_nodes' infrastructure/environments/*/terraform.tfvars`,
  read 2026-10-24), so `module.execution_node` is empty everywhere and no
  process would ever be handed the file. A configuration path no deployment
  sets is the shape the COMPLETE bar's part 2 refuses to count, so wiring it
  would add a parser, a refusal path and a startup-script line without
  moving the row.
- **Its contents are an owner's decision.** A register names, per
  instrument, the venue rulebook entry, the product approval and the legal
  opinion each permission rests on. No such reference exists for any
  instrument the node would trade, and a fixture register shipped in the
  startup template would be exactly the "permission nobody can trace"
  `CreationPermissions::record` exists to refuse.
- **The prohibition does not depend on it.** The requirement is that a
  market exists only where all three authorities permit it. On every shipped
  path that is held structurally: the gateway's only listing route,
  `ensure_listed`, asks the register first, and the venue's
  `list_synthetic` asks again for both admissions before it builds a
  record. With no register, nothing is created — which satisfies "only
  where governance supports them" by creating nothing.

## Consequences

- R12-COVERAGE-5107's prohibition is held on the node's default path by a
  guard built at the only place an instrument can come into existence there,
  which is the bar's part 3 "COMPLETE" shape for a prohibition.
- The node cannot trade any instrument on its own default path. Every
  pass-time behaviour the edge plane's telemetry describes for a placed order
  is reachable only in test, which `observability.md` already records for the
  node as a whole: no node is deployed.
- A deployment that needs the node to list must change code, not
  configuration, and that change is reviewed.

## What it costs

- The admitting half of the gate has no production caller. A regression
  that broke admission while keeping refusal would be caught only by the
  tests named above, never by a running node.
- When a node is deployed, the register's wiring, its file format and its
  refusal path are new work on the critical path of that deployment rather
  than code already reviewed.

## What would make this wrong

- **A node deployed without a register.** If `execution_nodes_enabled`
  flips to `true` in any environment while this record stands, the node
  would start, refuse every listing, and trade nothing while reporting
  healthy. The deploying change must wire the register; that is the reversal
  condition, not an option.
- **A test or a document that reads the admitting tests as evidence of a
  deployed capability.** The tests prove the gate; they are not a default
  path.
- **The owner deciding the gateway should list under an opt-in register
  now.** The shape is fixed here so that the work is small: a variable read
  in `qip-edge-node/src/main.rs` only, naming a JSON file of
  `qip_kernel::market_factory_stage::PermissionEntry` rows and a
  jurisdiction; unset meaning no register (today's behaviour); a file that
  cannot be read, does not parse, names an unknown action or authority, or
  records a permission without a reference stopping the process at start-up
  with exit 78 naming the reason, never lowered to an empty register; and the
  register handed to the gateway through `permit_listings` before the first
  pass.

## Dissent

Wiring the opt-in register now would give the admitting half a production
caller, and the parser it needs is the one the deep brain already uses for
its breadth plan. The lane's answer is that a caller no deployment reaches is
not the production evidence the dissent wants, and that the register's
contents — the evidence each permission rests on — are what make it
meaningful, and none exist. The dissent is right that the deployment change
will be larger for this record.

## Code it constrains

- `backend/crates/apps/qip-edge-node/src/gateway.rs`: `SimulatedGateway::new`
  builds with `listing: None`; `NodeGateway::open` calls it.
- `backend/crates/apps/qip-edge-node/src/main.rs`: reads no listing register.

## Not decided here

Any instrument's permissions; any register file's location; any deployment
of an execution node; any change to `infrastructure/**`.
