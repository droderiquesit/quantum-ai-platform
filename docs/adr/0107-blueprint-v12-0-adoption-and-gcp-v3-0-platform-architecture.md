# ADR 0107: Blueprint v12.0 adoption and GCP v3.0 platform architecture

- **Status**: Proposed for adoption, pending blueprint v12.0 and v3.0 availability
  and validation against the current codebase. When adopted, the owner's
  instruction will move this to **Accepted**.
- **Date**: 2026-10-06
- **Supersedes**: ADR 0099 (direction only; the paper-trading boundary and all
  standing decisions remain in force per ADR 0099's "What this does not do")
- **Amends**: `.claude/rules/10-product-direction.md` (architecture of record
  only)
- **Related**: ADR 0099 (v11.6 and v2.1 adoption), ADR 0100 (event fabric), ADR
  0003, 0021, 0023 (paper trading; untouched)

## Context

ADR 0099, adopted on 2026-09-25, made Algorik Master Blueprint v11.6 and GCP
Platform Blueprint v2.1 the architecture of record. On that date the owner
supplied three verbatim documents with stable SHA-256 hashes, held in
`docs/blueprint/source/`.

On 2026-10-06, the next generation of the blueprints is known to be in
preparation:

- **Algorik Master Architecture & Application Blueprint v12.0** — the next
  iteration of the application blueprint, expected to supersede v11.6's
  direction.
- **Algorik GCP Full Platform Architecture Blueprint v3.0** — the companion
  cloud/platform blueprint, expected to supersede v2.1's specific GCP
  recommendations.

This record prepares the adoption decision framework. It does not authorise
execution or any code change. It names what will change when v12.0 and v3.0
become available, and it registers the conflicts that will need resolution
through new standing decisions, in the pattern ADR 0099 established.

## Decision

When v12.0 and v3.0 blueprints are received, frozen, and their SHA-256 hashes
recorded:

1. **v12.0 and GCP v3.0 become the architecture of record**, in the same
   precedence as established by ADR 0099: v12.0 governs what the system does,
   v3.0 governs where and how it runs on Google Cloud. Where v3.0 is more
   specific about a v12.0 requirement, the more specific statement is the
   requirement.

2. **Traceability is structural, not narrative.** The documents will be broken
   into atomic requirements with stable IDs (`DOMAIN-NNN`) in
   `docs/blueprint/requirements/*.json`, rendered to
   `docs/blueprint/requirements.md`. Every ID gets exactly one row in
   `docs/blueprint/traceability-matrix.md`, and a row is COMPLETE only when
   its required behaviour is implemented *and* demonstrated by a named test or
   run.

3. **v11.6 and v2.1 become the historical register.** They are not deleted.
   ADRs that were decided against them remain in force as descriptions of what
   runs today and as standing decisions, until a record supersedes each one.

4. **The blueprint versions are held in `docs/blueprint/source/`** with the
   same protocol: verbatim PDFs, `.txt` derivatives, and interactive diagrams
   if provided, with `docs/blueprint/README.md` recording the SHA-256 of each.

5. **A conflict register will be compiled** on the pattern of ADR 0099 Table 1,
   listing every standing decision that v12.0 or v3.0 contradicts. Until each
   conflict is resolved by its own ADR, the standing decision keeps its force.
   Conflicts requiring owner decision (new autonomy boundaries, new capital
   exposure, new external dependencies) are blocked until the owner decides
   through a new ADR or amends a rules file explicitly.

6. **Paper trading is untouched.** Every requirement needing live capital,
   external action, or integration with a real trading venue is scored BLOCKED
   and built only in shadow, paper, or simulated form, under the same gates as
   ADR 0099 names.

## What this does not do

Carried forward from ADR 0099, because the hazard is the same.

**This decision authorises no execution whatsoever.** It settles the framework
for adopting the next blueprint. It does not provision, migrate, decommission,
add a dependency, open a path, or change configuration, and nothing in it may
be cited as permission to.

- **A standing decision is not overridden by a target that contradicts it.**
  Each conflict below gets its own record, with its own costs and reversal
  conditions, before any code acts on it.
- **Cloud work still goes through the gates it went through before.** ADR
  0040's workflow rule and cost posture apply. ADR 0093's owner cost
  instruction applies.

## Known structure changes from v11.6 to v12.0

The blueprints themselves will define what changes. Until they arrive, the
following is recorded as context for the programme planning process:

- **Candidate new components** based on the draft roadmap circulated:
  - Scout Fabric (active intelligence sensor)
  - Evidence/Truth subdomain (knowledge verification)
  - Tick Lake (temporal market state archive)
  - World Model Federation (distributed reasoning)
  - Causal Agency surface (autonomous decision publication)
  - Expansion Engine (opportunity search)
  - NOW Brain and Forecast Lattice (temporal reasoning)
  - Model Tournament and Forecast Market (ensemble benchmarking)
  - Peer-to-peer Reflex Mesh (cell coordination)
  - Federated Specialist Brains (distributed expertise)
  - Async Quantum Compute subsystem (deferred quantum evaluation)
  - Independent Risk Control systems (regional autonomy)
  - Event Fabric Consolidation (unified streaming)
  - Warm-tier Coordination Layer (service orchestration)

- **Candidate standing decisions that will need ADRs:**
  - ADR for warm-tier coordination layer (service placement, scaling,
    orchestration)
  - ADR for peer-to-peer reflex mesh (cell-to-cell communication,
    consistency)
  - ADR for federated specialist brains (brain placement, federation
    protocol)
  - ADR for async quantum compute (deferred quantum job scheduling and
    attribution)
  - ADR for independent risk control systems (regional autonomy, consensus)
  - ADR for event fabric consolidation (migration from current event fabric)
  - ADR for NOW Brain and forecast lattice (temporal reasoning architecture)
  - ADR for temporal forecast market (ensemble benchmarking, incentives)
  - ADR for model tournament infrastructure (multi-model competition,
    selection)

- **Known dependencies likely to surface:**
  - Async/await runtime (blocked by C2 in ADR 0099)
  - TLS and mTLS for inter-region communication (blocked by C2)
  - Distributed consensus mechanism (blocked by C2)
  - Managed search/analytics services (blocked by C4)
  - Real-time feature computation (blocked by C5 if Python-based)

These will be confirmed against the actual v12.0 and v3.0 documents when
received.

## What it costs

- **A larger standing gap again.** v12.0 adds to the already-large v11.6 gap.
  The honest completion percentage will drop further, even though no code got
  worse.
- **Approximately 9-10 new ADRs will be needed** to resolve conflicts that v12.0
  introduces. These are estimated from the known structure changes above, but
  the final count will be determined by the conflict register.
- **Traceability matrix recompilation.** Once v12.0 and v3.0 are frozen, the
  requirement decomposition, JSON structure, and traceability rows will be
  regenerated against the new document. Work already done will be re-scored
  against the new target.

## Alternatives rejected

- **Treat v12.0 as an incremental refinement and adopt it without a record.**
  ADR 0099's "What would make this wrong" says "A v11.7 or v2.2 arriving [does
  not] become the architecture of record by existing. That takes the owner,
  again." The same applies to v12.0. An adoption decision is required.
- **Wait until all v12.0 conflicts are resolved before adopting it.** The
  adoption and the conflict resolution are separate decisions. Adoption settles
  what the target is; conflict resolution settles how to get there. Waiting for
  resolution would stall the programme on preliminary questions rather than
  moving the target stable so work can proceed against it.
- **Adopt v12.0 and immediately deprecate v11.6.** v11.6 will remain the
  reference for the current codebase and the pattern of how to decompose a
  blueprint into requirements, until code has demonstrably moved to v12.0. The
  transition is scored, not assumed.

## What would make this wrong

- **Any live-order, live-transfer, purchasing, market-creation or
  public-communication path appearing** on the reasoning that v12.0 names it.
  That inference is invalid here as it was under ADR 0099.
- **A dependency, cluster, or managed service appearing in the tree before its
  record.** v12.0's adoption is not the reopening of ADR 0002, 0009, or 0024.
- **A blueprint beyond v12.0 arriving and being adopted without a record.**
- **Conflict register rows being amended without corresponding ADRs.** A conflict
  stays open until its own record exists and is accepted.
