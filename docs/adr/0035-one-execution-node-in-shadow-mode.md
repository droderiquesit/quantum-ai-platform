# ADR 0035 — One execution node, in shadow mode, in one region

- **Status:** accepted
- **Date:** 2026-09-04
- **Decides:** whether to deploy execution nodes at all
- **Relates to:** ADR 0024 (one node per region), ADR 0008 (cells decide
  alone), ADR 0003 (paper trading)

## The decision

Deploy **exactly one** execution node, in `us-east4`, in shadow mode, in the
`dev` environment. Not seven. Not one per region. One.

`execution_nodes` stops being `{}` in dev and stays `{}` in test, stage and
prod until this one has run for a sustained period and something has been
learned from it.

## Why one, and why not zero

**Zero is the status quo and it is not sustainable.** The entire edge plane —
the feasibility gate, netting, internal crossing, the pricing policy, two
independent halt wires, per-region capital reservation, the arbitrage desk,
confirmed-fill booking and reconciliation — exists, is tested, and runs
**only under `cargo test`**. `qip-edge-node` runs passes only when
`QIP_VENUE_FEED=simulated`, and no node is deployed anywhere, so the
pass-time series reach no process outside the test binary.

That is a large, safety-critical subsystem whose deployment behaviour is
entirely unobserved. Every week it stays that way, more code is written
against assumptions nothing has tested. The Phase 8 gate is about allocation,
allocation is the cell's job, and a gate argued from a cell that has never
run in a deployment is a gate argued from a test fixture.

**Seven is the blueprint's target and it is premature.** Seven regional cells
is the steady state, and going there first would multiply every
first-deployment surprise by seven, in seven regions, with seven journals and
seven sets of reconciliation breaks — while teaching nothing that one node
does not teach first. ADR 0008's argument is about partition behaviour, which
is a property of the second node onward; it is not an argument for standing
up all seven before the first has booted.

One node in one region answers the questions that are actually open: does the
composition root come up, does it prove its journal writable before reporting
healthy, do the halt wires fire in a deployment, does the Ops Agent scrape
its health port, do the pass-time series move against a real clock over days
rather than milliseconds.

## Shadow mode, and why that is not a hedge

The module defaults to shadow mode and this deployment keeps it. The node
observes, decides, records and does not act. Combined with paper trading —
which is absolute and enforced by three independent layers, none touched here
— that means the node is running the full decision path with two independent
reasons why nothing reaches a venue.

This is deliberately belt and braces. The first deployment of a subsystem
that has only ever run in tests is exactly where an unknown surfaces, and the
cost of shadow mode is that the first weeks produce decisions nobody acted
on. That is the intended purchase: a record of what it *would* have done,
which is the input the LEARN stage scores against and the only honest way to
find out whether the cell is right before letting it matter.

## What it costs

- **It bills while idle.** A Compute Engine instance is the one thing in this
  deployment that costs money whether or not it is doing anything; a Cloud
  Run service at zero instances does not. `infra.yml`'s targeted `down` exists
  precisely for this and remains the way to stop it.
- **A stateful thing to operate.** The node has a journal on a disk, a
  snapshot schedule that must be attached after first boot and after every
  replacement (the `journal_snapshot_attachment_command` output exists for
  this and is not automatic), and a failure mode Cloud Run does not have.
- **One region is not a test of the thing ADR 0008 is about.** Partition
  behaviour, the centre-to-cell policy path under loss, and per-region
  reservation contending across regions are all properties of more than one
  node. This deployment cannot exercise them and must not be described as
  having validated them.
- **It widens what is deployed while three central services are still the
  only things proven serving.** More surface, at a moment when the deployment
  story has only just started working.

## What would make this wrong

**If the node cannot be observed.** Deploying it before ADR 0032's collector
means standing up the least-observed subsystem in the platform with no way to
watch it — which is the failure this decision is meant to end, repeated in a
new place. The node's startup script already declares an Ops Agent Prometheus
receiver on its health port; that receiver reaching something is a
precondition, not a follow-up.

**If shadow mode is ever lifted as part of "making the node useful".** It
comes off as its own decision, with its own record, on evidence from the
shadow period. Not incidentally, and not to unblock a gate.

**If a second node is added before the first has taught anything.** The entire
argument for one is that it is a probe. Two nodes deployed a week apart with
nothing learned in between is just the seven-node plan taken slowly.

**If `execution_nodes` becomes non-empty in prod on this record.** It
authorises dev, and nothing else. Prod requires a human dispatch and its own
approval, as it always has.
