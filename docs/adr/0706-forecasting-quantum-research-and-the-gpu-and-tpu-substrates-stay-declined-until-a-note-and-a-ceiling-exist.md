# ADR 0706: Forecasting quantum research and the GPU and TPU substrates stay declined until a note and a ceiling exist

- **Status**: Accepted 2026-11-16 under decision 40 of
  docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of
  2026-11-15). It amends no Accepted record and touches no risk limit, capital
  control, billed infrastructure or paper-trading layer. It applies ADR 0150,
  ADR 0190, ADR 0158 decision 5 and owner decision 5 of 2026-10-25 to three
  rows and builds nothing.
- **Date**: 2026-11-16
- **Rows**: R12-QUANT-4054, R12-QUANT-9043, R12-QUANT-9044

## Context

Three open rows ask for things the platform does not have:

- R12-QUANT-4054 asks that quantum kernels, reservoir or QML models used for
  forecasting are checked by out-of-time calibration against a strong
  classical model. The register says no quantum path exists for this research
  domain, so the check does not exist either.
- R12-QUANT-9043 asks for GPU pools (Vertex custom training) serving training,
  multimodal inference and simulation, with Spot for interruptible jobs. The
  register records no GPU resources or job specs and `enable_vertex_ai = false`
  in the dev tfvars.
- R12-QUANT-9044 asks for TPU7x capacity on Compute Engine, scheduled by the
  compute router. The register records no TPU resources, and the row itself
  notes that the JAX or PyTorch stack it names is not Rust.

For 4054, ADR 0150 decision A2 permits a new family's encoding, baseline and
benchmark only after a committed note shows the sizes the platform poses lie
above the exact-enumeration limit and at or below the provider's qubit ceiling.
ADR 0190 confirmed it. ADR 0704 applies it to four sibling rows. A parked
patch in `docs/blueprint/lanes/L4-quant-research-families-adr0150.patch` holds
a reservoir model with out-of-time error and interval coverage reported beside
a classical model. It is not landed, and no note exists.

For 9043 and 9044, owner decision 5 of 2026-10-25 declines new billed
infrastructure, naming Vertex GPU and TPU, and reopens each by an ADR stating a
measured need and a monthly ceiling. ADR 0158 decision 5 keeps the `Tpu`
budget at literal zero with no consumer. The owner ruling of 2026-10-31 left
both rows open for that reason.

## Decision

1. **R12-QUANT-4054: decline.** The forecasting family stays not built under A2
   and ADR 0190. A forecasting model is not a combinatorial problem with an
   instance size, so the benefit note A2 asks for has nothing to measure until
   a forecasting question is posed that a classical model cannot answer. The
   row is graded against the declined clause. The parked patch is neither
   landed nor deleted. If a forecasting experiment is ever admitted, its
   record must carry out-of-time calibration for the quantum model and for a
   strong classical model side by side, and the classical model must be
   chosen without sight of the quantum result, in keeping with ADR 0006.
2. **R12-QUANT-9043: decline.** Vertex GPU training stays declined under owner
   decision 5. No measured need and no monthly ceiling is on record, and the
   Vertex port has no client, credential or egress path. This record sets
   none. The row's evidence line should read `terraform.tfvars:382` as of this
   date, not the line the register carries; recount rather than quote.
3. **R12-QUANT-9044: decline.** TPU stays out of scope under ADR 0158 decision
   5 and owner decision 5. A TPU7x slice is billed capacity with no consumer,
   and the training stack it implies is a non-Rust dependency that needs its
   own ADR under ADR 0109 before any code names it. This record admits no
   dependency and creates no reservation or router arm.
4. **Need a figure: the monthly ceiling for accelerator capacity.** Both 9043
   and 9044 reopen on an ADR stating it. No ceiling is chosen here, because
   any number written without a measured workload would be invented.
5. **Need a figure: the measured accelerator-shaped workload.** It is the
   wall-clock and cost of a named training or simulation job on the CPU
   capacity the platform already has, taken from a committed run. Without it
   "slow and expensive on CPUs", the row's rationale, is an assertion.
6. **The owner holds the commitments.** A GPU or TPU budget, a Vertex
   credential and any billed hardware time need the owner. The orchestrator
   does not hold them under decision 40.

## What it costs

Three rows stay open, and 9043 and 9044 stay MISSING for an indefinite time.
Training and simulation keep running on the capacity the platform has, however
slow that is, because nobody has measured whether it is slow enough to matter.
The research desk cannot try a quantum forecaster even as a tie. The parked
patch drifts from the tree it applied to and will need re-applying and
re-running if a case appears. Decisions 4 and 5 ask for numbers nobody has
taken, so the rows cannot move in either direction until they are taken.

## What would make this wrong

If a committed measurement shows a platform job whose cost or duration on CPUs
is a binding constraint on the research desk, the declination of 9043 is wrong
and the measurement is the evidence an ADR with a ceiling needs. If the owner
grants a ceiling, or supersedes owner decision 5, decisions 2 and 3 no longer
hold. If a forecasting problem is posed whose size lies above the enumeration
limit and within the simulator, decision 1 no longer holds, and ADR 0150's own
route (the benefit note) is the way to reopen it. If the Rust-only position
(ADR 0001) is relaxed for the accelerator stack, decision 3's dependency
condition is the one to revisit.
