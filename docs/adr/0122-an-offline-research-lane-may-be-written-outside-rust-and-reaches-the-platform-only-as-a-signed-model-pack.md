# ADR 0122: An offline research lane may be written outside Rust, and reaches the platform only as a signed model pack

- **Status**: **Accepted**, 2026-10-11, by the owner in the session, under the
  instruction: "You make the recommended decision and have full permission".
- **Date**: 2026-10-11
- **Resolves**: ADR 0099/0101 conflict C5 ("a research-runtime record"), for
  research only.
- **Amends**: ADR 0001 and ADR 0011 ("Rust everywhere") by one bounded
  exception beside the browser layer. Every service, every binary, the
  decision core and the cell stay Rust. `.claude/rules/10-product-direction.md`
  names the browser as "the only exception"; that sentence needs the owner's
  edit to name this one, which this record does not make.
- **Keeps**: ADR 0083 (a trained model is served in-process by the in-tree
  provider, for the forms the platform trains), ADR 0110 (model packs are
  signed by Cloud KMS and verified at the edge), ADR 0006 (a classical
  baseline every time), ADR 0102 (the fleet's spend ceiling), ADR 0114 D4 (no
  workload on Kubernetes).

## Context

R12-MODEL-1082 is BLOCKED because all training is in-tree Rust
(`qip-training`), a Vertex AI port exists with `enable_vertex_ai = false`, and
ADR 0001/0011 refuse a non-Rust lane. At real-money scale the research desk
will want the numerical ecosystems where model research actually happens,
and rebuilding each estimator in Rust before it can be tried makes the
platform slower to learn without making it safer: the safety properties —
what may run in a decision path, what a cell may load, whether a result is
reproducible — are properties of the *artefact and its admission*, not of the
language the experiment was written in.

## Decision

1. **A research lane in Python is permitted, offline only.** It lives in its
   own top-level directory with its own pinned lockfile (hashes required),
   runs only as Vertex AI custom jobs or on a developer's machine, and has
   **no network path to any production service, fabric, ledger or cell** and
   no credential that could reach one. It reads exported, licensed datasets
   (licensing evaluated first, as for every source) and writes artefacts to
   its own bucket.
2. **Its only output into the platform is a model pack** in a form the
   in-tree provider already serves (ADR 0083), carrying its training data
   digest, code commit, container digest and the classical baseline it was
   measured against (ADR 0006). The pack is admitted only through ADR 0110's
   two-person, KMS-signed publish; a cell or brain loads only a signed pack,
   never a lane's output directly. A form the provider cannot serve is not
   admitted by this record; serving a new form is a Rust change under ADR
   0083.
3. **The hot lane and every service stay Rust.** No Python process is ever a
   deployed service, sidecar, or step in a cycle.
4. **Accelerators are reached here, and only here, as Vertex AI custom jobs**
   — not as GKE node pools (ADR 0114 D4) — and only when a committed
   measurement shows the CPU path inadequate for a named experiment (ADR
   0069).
5. **Spend is held under ADR 0102's existing ceiling** until a record states
   a separate one. `enable_vertex_ai` is flipped per environment, with the
   plan shown, and stays false in stage and prod.

## What it costs

- **A second language and a second supply chain** (PyPI) with its own
  advisories, which the dependency policy must govern by the same rule as
  `Cargo.toml` and `package.json`: every addition reviewed, the transitive
  tree part of the diff.
- **A translation step** from what research produced to what the provider
  serves; a model that only exists in a Python framework cannot ship.
- **Reproducibility work**: pinned containers and dataset digests are
  required, or a pack's provenance is a claim rather than a record.
- **Cloud spend** for training jobs, inside the existing ceiling.

## Alternatives rejected

- **Keep research in Rust only.** Every experiment pays the cost of a
  production-grade reimplementation before it can be falsified.
- **Python services in the decision path** (v12.0's shape for forecasting
  and twins). Refused: it reopens ADR 0001 where its reasons are strongest
  and puts an interpreter beside capital.
- **GPU node pools on GKE.** ADR 0114 D4.

## What would make this wrong

- Any network route or credential from the research lane to a production
  service, the fabric, the ledger or a cell.
- A model loaded by a cell or a brain that did not pass ADR 0110's signed
  publish, or a pack without its data digest, commit and baseline.
- A Python process deployed as a service or run inside a cycle.
- Research spend outside the stated ceiling.

## Dependency direction

Nothing in `backend/` depends on the research lane, and the lane depends on
nothing in `backend/` except the published model-pack format, which it
writes as data. No crate is added; `qip-training` and the provider are
unchanged. No lib depends on a service; no service depends on the runtime;
nothing depends on an app.

## R12 ids

Unblocks to build: **R12-MODEL-1082**. Decides the accelerator placement for
**R12-MODEL-10065** (Vertex AI custom jobs), which stays BLOCKED until an
experiment measures a need; the burst half of **R12-GCP-9015** is the same
placement.

## The paper-trading boundary

Untouched. A research artefact reaches a cell only as a signed pack through
reviewed publish, and a pack sizes nothing beyond the envelope it is given;
the three layers, `Determinism::Required` and the ledger's fourth fence stand.

## Gates

None run. Markdown only, and no shell in the session that wrote it.
