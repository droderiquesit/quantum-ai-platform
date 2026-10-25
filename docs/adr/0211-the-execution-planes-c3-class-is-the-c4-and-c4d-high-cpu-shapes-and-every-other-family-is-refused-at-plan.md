# ADR 0211: The execution plane's C3 class is the C4 and C4D high-CPU shapes, and every other family is refused at plan

- **Status**: **Accepted**, 2026-10-24, decided by lane L6 under the owner's
  delegation of the same day (`docs/blueprint/owner-decisions-2026-10-24.md`).
  It changes no code and no environment; it records which of two requirement
  texts governs a check they state differently.
- **Rows**: R12-GCP-7002, R12-REFLEX-8002.

## Context

R12-GCP-7002 (GCP v2.1) asks that Reflex Cells run on "dedicated GCE
C3/C3D-class (or equivalent) low-latency VMs", and its check reads "a
non-C3 machine_type is refused". R12-REFLEX-8002 (GCP v3.0 §1, the later
blueprint) asks for C4/C4D-class machines and says "a C3 or N2 shape must
be refused at plan". `modules/execution-node/variables.tf` implements the
later one: `machine_type` admits only `c4-highcpu-8`, `c4-highcpu-16`,
`c4d-highcpu-8` and `c4d-highcpu-16`. Read literally the two checks cannot
both hold: one refuses everything but C3, the other refuses C3.

## Decision

1. The execution plane's machine class is the C4 and C4D high-CPU shapes
   above. They are the successor generation of the C3/C3D compute-optimised
   family and are the "equivalent" R12-GCP-7002's own text admits.
2. R12-GCP-7002's check is read as "a machine outside the execution
   plane's class is refused at plan", which the module's validation already
   does for every family including C3, C3D and N2.
3. No fallback to C3 is kept. A shape nobody chose is not a fallback; if
   the C4 family is unavailable in a region, that region is not an
   execution region until a record says otherwise.

## What it costs

- A region offering C3 and not C4/C4D cannot host a node. The single
  execution region (ADR 0035) offers both.
- R12-GCP-7002 remains PARTIAL for its other clause: no environment has
  rendered a node in a real plan (`execution_nodes_enabled = false` in dev,
  no node elsewhere), and enabling one is a protected, billed change.

## What would make this wrong

A measured latency or isolation property that C4/C4D lacks and C3 has.
Then the class changes here, with the measurement cited.

## Evidence

`infrastructure/terraform/modules/execution-node/variables.tf`
(`machine_type` validation); the execution-node module's mocked harness.
