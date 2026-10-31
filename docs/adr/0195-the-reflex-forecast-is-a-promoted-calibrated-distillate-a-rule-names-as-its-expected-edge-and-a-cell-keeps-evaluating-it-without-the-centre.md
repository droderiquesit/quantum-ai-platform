# ADR 0195: The reflex forecast is a promoted, calibrated distillate a rule names as its expected edge, and a cell keeps evaluating it without the centre

- **Status**: **Accepted**, 2026-10-31. Decided by lane L4-quant under the
  parent's 2026-10-31 delegation
  (`docs/blueprint/owner-decisions-2026-10-31.md`, decision 2). That
  decision lets a lane decide a row that waited only on a product decision.
  The number was claimed on 2026-10-24, and decision 15 of 2026-10-31 left
  it in place. The 2026-10-31 delegation does not lift four limits:
  - no live-order path;
  - no real capital or money movement;
  - no weakened test or limit;
  - no new dependency.

  This decision needs none of them.
- **Date**: 2026-10-31
- **Keeps**:
  - ADR 0008: cells decide alone, and no model is consulted over the network
    on the reflex path.
  - ADR 0083: a trained model is served in process.
  - ADR 0024: one execution node per region.
  - ADR 0003: paper trading.

## Context

R12-WORLD-9023 asks for microsecond-to-minute microstructure forecasts. They
must be calibrated, run locally in the reflex cell with a warm backup, and
continue independently of central services. Its acceptance is an
integration test: the cell evaluates a local model in its pass with no
network call, and central unavailability does not stop it.

The row was graded as having no forecast model under `edge/`. Read against
the code, every part of the mechanism is already in the tree:
- A strategy rule can declare its expected edge as any expression
  (`Rule::with_expected_edge`), including an inline model
  (`Expr::Model`). The compiler carries that model into the plan as
  `Op::Model`.
- `Cell::work` evaluates the plan in the pass.
- A `DistilledModel` carries its own calibration (`with_calibration`), and
  the calibration is inside the model's digest.
- A plan carrying a model installs only when the signed policy's
  `trained_models` manifest names that model at that digest
  (`check_models_promoted`).
- The centre fills that slot from the kernel's promoted set, in `qip-api`'s
  mesh.
- The net-edge gate (`net_edge_refusal`) refuses a risk-increasing signal
  whose stated edge is below its fixed cost plus a margin.

What was never decided is which forecast this is, how its calibration
ships, what it may do, and what "independently of central services" means
for a model whose admission the centre signed.

## Decision

1. **The reflex forecast is a promoted distillate that a rule names as its
   expected edge.** It is a `DistilledModel`, an `Expr::Model` over the
   feature DAG's microstructure features, declared through
   `Rule::with_expected_edge`. It forecasts the edge in basis points over
   the seconds-to-minutes horizon. It is evaluated in `Cell::work`, in
   process, with no call that leaves the cell. No other forecast form is
   added for the reflex lane.
2. **Its calibration ships on the model and is admitted with it.** The
   calibration is applied with `DistilledModel::with_calibration` and is
   part of the model's digest. The centre promotes the model through
   `Platform::promote_model_from` and names it in the `trained_models`
   manifest, and the cell admits the plan only at that digest. A
   recalibration is therefore a new digest, promoted and shipped like any
   model, and never a number a cell adjusts on its own.
3. **The forecast may only refuse.** Its one effect on a decision is the
   existing net-edge gate. A signal whose forecast edge is below its fixed
   cost plus the configured margin is refused before netting. Nothing reads
   the forecast to enlarge an order, to choose a venue or to change a
   limit. A gate that can only refuse cannot widen the paper boundary
   (ADR 0003), and a forecast that is wrong costs at most a trade not
   taken.
4. **A cell keeps evaluating the forecast without the centre.** The plan
   and its model stay installed on the last policy the cell applied.
   `check_models_promoted` deliberately does not consult the manifest's
   freshness: a stale manifest is still the last set the centre signed.
   The policy's age narrows the cell's capability table elsewhere, but it
   does not unload the model. A centre that stops answering therefore
   leaves the forecast running on the last promoted calibration.
5. **"Warm backup" is declined as a second node.** A standby execution node
   per region is billed infrastructure that ADR 0024 does not provide, and
   the delegation does not cover new paid infrastructure. The requirement's
   warm-backup clause stays unmet. A restarted node recovers its state in
   the way it already does, and this decision adds nothing to that path.

## Consequences

- R12-WORLD-9023 is graded on an integration test that drives `Cell::work`
  with a calibrated model as a rule's expected edge, after the policy's
  `valid_for` has elapsed and with no link to the centre. The model's
  forecast must reach the signal, and the net-edge gate must refuse the
  signal whose forecast is below cost. The warm-backup clause stays open.
- No committed production strategy declares a model as its expected edge
  yet. Shipping one is a strategy decision for the desk, and nothing here
  makes it.

## What it costs

- A forecast that is stale by design. With the centre unreachable, the cell
  runs on the last promoted calibration for as long as the outage lasts,
  and the calibration drifts from the market it describes. That is the
  price of "continues independently"; the gate it feeds can only refuse,
  which bounds it.
- No warm standby. A node that fails is a region without a reflex lane
  until it restarts.
- One more way for a trade not to happen. An over-pessimistic calibration
  refuses edges that were real; the net-edge refusal counter is where that
  shows.

## What would make this wrong

- A measured case where a stale calibration refuses materially more good
  trades than a fresh one would. Then a staleness bound that withdraws the
  forecast (reading it as no edge, which the gate already handles) should be
  added, and this decision amended.
- An owner decision to fund a standby node per region. Then the
  warm-backup clause needs its own record.
- Any proposal to let the forecast enlarge or route an order. That changes
  decision 3 and needs its own ADR, read against ADR 0003 first.
