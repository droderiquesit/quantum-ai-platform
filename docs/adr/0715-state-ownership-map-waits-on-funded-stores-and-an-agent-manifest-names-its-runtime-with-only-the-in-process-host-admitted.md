# ADR 0715: State ownership map waits on funded stores, and an agent manifest names its runtime with only the in-process host admitted

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-ARCH-8029, R12-ARCH-9028

## Context

R12-ARCH-8029 requires explicit state ownership: Spanner owns financial
truth, the Fabric owns event history and offsets, GCS owns retained archives
and derived knowledge artifacts, Bigtable owns hot derived features, and
external payloads are transient. Its check is that each fact class maps to one
store in code and that a second writer of a class fails an architecture test.
Today the Fabric owns history and offsets (`commit_offset` in the fabricd
broker). Financial truth lives in `qip-ledgerd`'s local `DurableStore` and the
kernel ledger. The GCS archive is unwired and Bigtable is absent. The owner's
2026-10-31 second-pass ruling keeps the requirement as written and declines to
map fact classes onto today's local stores so the row passes. ADR 0113
admitted Spanner for the ledger only. R12-LEDGER-7009 and R12-DATA-9053 hold
the Spanner and Bigtable decisions; both stay declined under decision 5 until
a deploy has run and a store record states the measured need and a monthly
ceiling.

R12-ARCH-9028 requires that the platform not force every agent onto one
runtime. Today `qip-agents` has one in-process Rust runtime and an
`AgentManifest` that does not name a runtime. The owner's ruling (decision 14
of the same document) does not admit Vertex AI Agent Engine: it is a paid
managed runtime that holds sessions and memory outside the event log, and
decision 5 keeps new billed services out. The ruling asks for an ADR that
makes the manifest name its runtime and states that the in-process host is the
only runtime admitted.

## Decision

1. **R12-ARCH-8029: the fact-class map is not written yet (owner).** The
   target mapping is the requirement's own text and is recorded here as the
   target, not as the state of the code. No agent maps financial truth or hot
   features onto the local stores. The row stays PARTIAL.
2. **R12-ARCH-8029: the trigger is stated.** When the Spanner ledger of
   R12-LEDGER-7009 and the Bigtable store of R12-DATA-9053 are each funded by
   their own accepted records and deployed, a lane writes the fact-class to
   store map in code. It adds a `qip-acceptance` architecture test that fails
   when a second writer of any class exists. That test is mutation-verified by
   adding a second ledger-class writer and confirming it fails.
3. **R12-ARCH-8029: the funding figures are needed, and none is chosen here.**
   The figures are the monthly ceiling for Spanner and the monthly ceiling for
   Bigtable, each named by its own store record. Until they exist, no store is
   enabled and no billed resource is created.
4. **R12-ARCH-8029: external payloads stay transient.** Nothing in this record
   permits a second copy of an external payload to outlive the event that
   carries it. This restates the bounded-retention rule and adds no behaviour.
5. **R12-ARCH-9028: build a runtime field in the manifest.** `AgentManifest`
   gains a required field that names the runtime the agent runs on. A manifest
   that omits it, or names a runtime the host does not know, is refused, not
   defaulted. The change is a contract change to `AgentManifest` and ships with
   its own test and its mutation check in the lane that builds it.
6. **R12-ARCH-9028: exactly one runtime is admitted, the in-process Rust
   host.** It is the only value the field accepts. The field exists so a
   second runtime is a deliberate, reviewed addition and not a silent
   assumption.
7. **R12-ARCH-9028: the second runtime is declined for now (owner).** Vertex AI
   Agent Engine is not admitted. Admitting it needs a funded record stating the
   measured need and a monthly ceiling, and a statement of how its sessions and
   memory are brought back under the event log so decisions stay reproducible.
   The row stays PARTIAL, because its check requires two supported runtimes.
8. **R12-ARCH-9028: nothing leaves the process.** Until decision 7 changes, no
   agent call reaches a managed service, and pre-trade deterministic checks
   remain outside any model or managed runtime.

## What it costs

The ownership row stays PARTIAL for as long as the stores stay declined, and
the single-owner architecture test does not exist until then, so a second
writer of financial truth is caught only by review. The manifest field adds a
refusal path: every existing manifest must name the runtime, which is a small
migration the building lane carries. The agent row also stays PARTIAL,
because one admitted runtime cannot satisfy a check that asks for two. Neither
decision touches a risk limit, capital, the paper-trading boundary or billed
infrastructure, and neither amends an Accepted record.

## What would make this wrong

- If the event log and the local ledger prove unable to carry financial truth
  at the measured load, the Spanner record should come sooner than this order
  assumes, and decision 2's trigger should be reread.
- If a second writer of a fact class appears before the stores exist, the
  absence of the architecture test is the failure, and a test over today's
  stores may then be worth its cost despite decision 1.
- If a use for sessions or memory appears that the in-process host cannot
  serve, decision 7 should be reopened with a measured need and a ceiling.
- If a required runtime field makes manifests harder to read than the second
  runtime ever justifies, the field was premature.
