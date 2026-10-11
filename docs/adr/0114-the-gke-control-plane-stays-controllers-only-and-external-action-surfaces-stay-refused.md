# ADR 0114: The GKE control plane stays controllers-only, and external-action surfaces stay refused

- **Status**: *Proposed*, 2026-10-11. The owner accepts in writing; nothing
  here takes effect by being written. It confirms standing positions rather
  than changing them, so acceptance moves no resource and no code.
- **Date**: 2026-10-11
- **Resolves**: owner decisions D4 and D8 in
  `docs/blueprint/v12-completion-plan.md`; ADR 0101 conflicts C3 and C7 for
  the present.
- **Confirms**: ADR 0036 (Argo CD and Kargo on a control-plane cluster that
  runs no trading binary), ADR 0093 (that cluster is suspended in `dev` while
  no Argo CD release passes the scan gate), ADR 0024 decision 1 (Cloud Run for
  warm binaries, one Compute Engine execution node per region), ADR 0091
  (binaries composed from libraries).

## D4 — Kubernetes

**Context.** The plan records D4 as "ADR 0024 / ADR 0036 disagree; CLAUDE.md
says none". They do not disagree once read with their dates: ADR 0036
superseded ADR 0024's decision 2 *for the controllers only* and kept every
trading binary off Kubernetes (`No Pod runs a qip-* image`), and ADR 0093
suspended the cluster without amending 0036. `CLAUDE.md`'s "no Kubernetes"
in the `infrastructure/` row is shorthand that is true of workloads and false
of the control plane. ADR 0101's C3 notes v3.0 places most of its
twenty-seven new services on regional GKE.

**Decision.** Keep it as it is. The control-plane cluster, when resumed,
runs Config Connector, Argo CD, Kargo and cert-manager and nothing else.
Every `qip-*` binary runs on Cloud Run or a bare Compute Engine node. Every
R12 row whose requirement places a workload on GKE stays BLOCKED by this
decision — not by an oversight — until a runtime record with a *measurement*
argues otherwise, as ADR 0091 requires for any claim of independent scaling.

**Why.** The real-money direction strengthens this rather than weakening
it. The execution node's founding property is that nothing sits between the
cell and the venue (`gcp_placement.rs` refuses an execution node behind a
load balancer or mesh); a Kubernetes node pool adds a kubelet, a CNI and a
scheduler to that path. And a Kubernetes API that can schedule a `qip-*`
image is a fourth way to change what trades, beside the three ADR 0036 already
guards.

**Rejected.** *Regional GKE Standard for warm services* (v3.0's shape):
re-argues ADR 0024 decision 1 — secrets as volumes, the trust zones'
network tags — for scaling nobody has measured. *Retire the control-plane
cluster entirely*: reverses ADR 0036, an owner decision, on cost grounds ADR
0093 already answered by suspension.

**What would make this wrong.** A `qip-*` image in any Pod spec. A
measurement showing Cloud Run cannot meet a warm service's latency or
concurrency need that a GKE deployment does. The cluster staying suspended
indefinitely — then ADR 0036's delivery path is a configuration nobody runs,
and a record should decide whether it exists.

**Instruction-file note.** `CLAUDE.md`'s `infrastructure/` row ("no
Kubernetes") should read "no workload on Kubernetes; the GitOps control-plane
cluster of ADR 0036 runs controllers only". That edit is the owner's to make.

## D8 — external-action surfaces

**Context.** Blueprint §24.4 asks for agents that publish to the outside
world, conduct outreach, price products and change operations. The register
scores six AGENCY rows BLOCKED, and they are two different kinds:

- **External action**: R12-AGENCY-6092 (publishing), R12-AGENCY-6093
  (outreach), R12-AGENCY-6095 (pricing a product), R12-AGENCY-6096 (operational
  change by an agent). `qip-agency`'s engine documentation already says an
  adapter needs the paper boundary and an ADR.
- **Live capital**: R12-AGENCY-6097 (allocation and hedging that move real
  capital and collateral), R12-AGENCY-6098 (live market actions), and
  R12-GOV-6119 (custody, transfer, banking credentials). These are **not**
  D8's. ADR 0003, ADR 0021 and ADR 0108 govern them, and this record does not
  touch them.

**Decision.** The four external-action rows stay refused, by design. No
adapter that publishes, sends, prices or deploys on an agent's initiative is
built; the `EXTERNAL_ACTION` refusal and the `AffordanceGraph`'s confinement
of operational edges to the owned-system registry stay as they are. Agents
may *draft* — a release note, a message, a price analysis — as an artefact a
person reads and acts on through their own credential.

**Why.** Each of these is an action the platform cannot take back and whose
consequences are not reproducible from the event log — the property ADR 0007
and the product direction put first. A real-money platform raises the cost of
an agent speaking or acting for the owner in public, it does not lower it;
and change-management rules already forbid an agent deploying, so §24.4's
operations surface would contradict them.

**Rejected.** *Build the adapters behind a disabled flag*: a refusal held by a
flag is one value from an action, the shape ADR 0003 and ADR 0108 reject for
live orders. *A human-in-the-loop approval per action*: viable later, and it
needs ADR 0108 P9's per-person, platform-unmintable approval credential
first; it gets its own record then.

**What would make this wrong.** An adapter in `qip-agency` that calls an
outside system on an agent's initiative. An agent-drafted artefact sent
without a person's act. The owner deciding the platform should act externally
— then a record per surface, with its own preconditions, supersedes this
section.

## Dependency direction

Nothing moves. No code, no crate, no Terraform. The D4 decision keeps
`apps/**` binaries deployed outside Kubernetes; the D8 decision keeps
`qip-agency` (service) free of any adapter crate reaching an outside system.

## R12 ids

D4: the GKE-placed G1/G2/G3 rows stay BLOCKED by decision (ADR 0101 C3/C7);
none is unblocked. D8: **R12-AGENCY-6092**, **6093**, **6095**, **6096** stay
BLOCKED by decision. **R12-AGENCY-6097**, **6098** and **R12-GOV-6119** are
listed so that they are not mistaken for D8 rows; they belong to ADR 0108.

## The paper-trading boundary

Untouched, and D8 is partly a statement of it: no agent-initiated external
action, and no live capital path. The four fences and `Determinism::Required`
stand.

## Gates

None run: no shell in the session that wrote this record.
