# Incident playbooks

Companion to [the operator runbook](../RUNBOOK.md); same labels (**ran**,
**repo**, **not run**), same date, 2026-10-04.

**Read this before you read any playbook.** No process of this platform is
running in any cloud environment, so none of these has been used in an
incident. Each is built from the code and the existing runbooks. Where a
drill was possible on a workstation I ran it and say so; where the component
cannot run at all I say that instead of writing a procedure for it.

One rule first, from [the operations index](../../operations/README.md):
**reducing autonomy needs no authority and tripping the kill switch takes
none.** If you are unsure, stop. A false stop costs far less than a missed
one, and clearing is the step that needs deliberation.

Also covered by their own pages, not repeated here:
[the kill switch tripped](../../operations/kill-switch.md),
[a risk limit is breached](../../operations/limit-breach.md),
[an agent attempted an ungranted capability](../../operations/permission-violation.md),
[a venue was withdrawn](../../operations/reinstating-a-venue.md).

## A cell is halted

**What it means.** A cell (`qip-edge-node`) has stopped placing new orders.
Cancels and confirms continue. There are up to four independent causes, and
each is a `source` label on `qip_edge_halted`:

| `source` | Cause | Who can lift it |
|---|---|---|
| `polled` | The flag file at `QIP_HALT_FLAG_PATH` is present, empty, unreadable, malformed, over 256 bytes, or its directory is missing | Whoever can write that path |
| `kill_switch` | A signed halt arrived on the mesh (broadcast from `qip-api`'s `POST /kill-switch` or the centre) | Engage-only: release rides a newer signed policy payload from the centre |
| `policy` | The applied policy payload says halted | The centre's next payload |
| `journal` | The spool-pressure wire; only a cell built with `CellConfig::with_journal_wire`, and **no composition root does** | Not applicable in this tree |

**Do this.**

1. **Do not clear anything yet.** Find out which wire is engaged:
   ```sh
   curl -s http://<cell health address>/metrics | grep '^qip_edge_halted'
   ```
   (**ran**: three lines, `kill_switch`, `policy` and `polled`, each 0 or 1.)
   The node log names the polled reason too (**ran**): `polled halt flag is
   engaged: <reason>; the cell is halted`.
2. Read the health body for the cell, region, `halted`, and `halt_flag`
   (`path`, `engaged`) (**ran**):
   ```sh
   curl -s http://<cell health address>/health
   ```
3. If `polled` is 1: read the file at the path in `halt_flag`. It holds the
   reason someone wrote. Ask them before deleting it.
4. If `kill_switch` or `policy` is 1: ask whether the central kill switch was
   tripped (`GET /api/v1/system/status` on `qip-api`, `halted` and `autonomy`).
   A centre halt is not lifted from the cell, and **no operator command in this
   tree issues the release payload**. Leave it halted.
5. Check what the cell is holding. A halt stops new orders and does not cancel
   working ones; see the working-orders step in
   [the kill switch tripped](../../operations/kill-switch.md).
6. Release a `polled` halt by deleting the flag (**ran** locally):
   ```sh
   rm <the flag path>
   ```
   On the execution node the path is `/run/qip/halt/engaged` (**repo**,
   `startup.sh.tftpl`; **not run**, no node exists). The cell reads the flag
   every pass, so it releases within one pass.

**If the flag directory vanished** the cell reads engaged: the mount that
carries the flag is gone and the wire's state is unknown. Restore the mount;
do not create a bare directory to make the halt go away unless you have
established why the mount left.

**Why it surfaces as an alert.** The `edge_halted` policy (any `source`, per
cell) is one of nine and is gated on `workload_metrics_exist`, so today it pages
nobody ([runbook section 9](../RUNBOOK.md#9-observability-what-is-emitted-what-is-scraped)).

## A reconciliation break

**What it means.** The platform's book and the venue's record disagree. Two
planes record it:

- The cell, from `qip_edge_reconciliation_breaks_total`: what it found.
- The centre, from `qip_central_reconciliation_breaks_total{direction}`:
  what it acted on. `direction` is `cell_over_venue`, `venue_over_cell`,
  `detail_only` or `unsent_fill` (a fill reported on an order the centre never
  saw sent, or beyond the quantity it saw sent). A centre break halts that cell.
  `qip-api`'s `/api/v1/health` carries `reconciliation_breaks` (**ran**: `0` on
  a fresh process).

**Do this.** The existing page is the procedure:
[the book and the venue disagree](../../operations/reconciliation-break.md).
In short: halt, read the breaks, get the venue's own record, correct the book
from the venue and never the other way round, clear the halt once the two
agree. Two corrections to that page, found while writing this one:

- Halt with the query-string form, which is the one the route reads (**ran**,
  HTTP 200): `curl -X POST -H "Authorization: Bearer $QIP_TOKEN_OPERATOR"
  "$API/api/v1/kill-switch?reason=reconciliation%20break%20under%20investigation"`.
  The page's `-d '{"reason": ...}'` is ignored and the halt carries the default
  reason.
- "Clear the halt" is not an API action today: `DELETE /api/v1/kill-switch`
  returns 403 and a restart is the only lift
  ([runbook 4.2](../RUNBOOK.md#42-halting-the-central-plane)).

There is no venue in any environment. The venue's record is a person's
account statement or a sandbox, and the break procedure has never run against
one.

## Fabric spool pressure

**This playbook describes a design, not a system.** The event fabric's broker
(`qip-fabricd`) refuses to start (**ran**, exit 1), the producer-side spool
that would feed it is specified in ADR 0100 sections 3 and 6 but is not wired
into `qip-edge-node` (the `event_fabric` module the ADR names is not in
`backend/crates/apps/qip-edge-node/src/`), and no composition root builds a
cell with `CellConfig::with_journal_wire` (**repo**, a search of
`backend/crates/apps` found no use). So there is no spool, no pressure reading,
and no `qip_edge_halted{source="journal"}` series in any process.

What the design says would happen, so that you recognise it when it exists
(ADR 0100 section 6): spool pressure is a fourth halt wire that is applied
every pass and **fails engaged**; it first narrows a pass's sizing to half and
then halts new exposure, before the spool budget is exhausted; cancels and
confirms continue; the decision thread never waits on the fabric. The ADR also
names the signal that something is wrong in a healthy run: spool bytes not
returning to baseline after archive.

**What to do today.** Nothing in a deployment, because nothing exists. If you
meet the symptom in a test or a local run, the playbook is the cell-halted one
above with `source="journal"`; do not clear it by writing the polled flag,
because the polled flag and the journal wire are independent and clearing one
does not release the other. When the fabric is built this page needs a real
procedure; it should be written from a run, not from the ADR.

## A ledger consumer stall

**There is no ledger consumer to stall.** `qip-ledgerd` refuses to start
(**ran**, exit 1): its `config`, `consumer` and `read_api` modules are stubs,
and nothing in Terraform or the catalogue references it. The pure double-entry
posting logic (`qip-portfolio::ledger`) and the store (`qip-ledgerd/src/store.rs`)
exist as libraries and are exercised by tests, not by a process.

What this means for you:

- The platform's "ledger" in any running process is the paper ledger inside
  `qip-api` (`paper_ledger.rs`) on the `memory` store; it dies with the process.
- Do not look for a consumer lag metric. None is emitted by a running
  process; `qip-ledgerd/src/telemetry.rs` defines series for a process that
  does not exist.
- When the broker and ledger exist, the failure to watch for per ADR 0100 is
  "the ledger posting a fill twice, or silently skipping one, under duplicate
  delivery or after broker loss and resend". Dedupe is by chain continuity.
  That is a design statement, not a measured behaviour.

## A failed deploy

**First, what "deploy" can fail at today.** Nothing reaches a running
service because no service exists in dev and the GitOps control plane is
suspended (ADR 0093). A failed `deploy.yml` run can therefore fail in the
pipeline half only: the gate, the build, the scan, the push, the attestation.
Read the failing job's log in the run's page on GitHub; I did not dispatch one.

**By stage:**

| Stage | Symptom | Do |
|---|---|---|
| `gate` | `no successful ci run for <sha>; nothing is deployed.` | Fix ci on that commit. Do not route around the gate |
| `gate` | `prod is not deployed automatically.` | Correct: prod needs a human dispatch and an approval; do not automate |
| `gate` / identity | an environment whose `project_id` is `unprovisioned` | Refused by design; provision a project first (runbook section 7) |
| `images` / `scan image` | `trivy image --severity CRITICAL,HIGH --exit-code 1` fails | Fix or upgrade the dependency. **Never** add a scanner exception to pass; ADR 0093 is exactly this failure for Argo CD |
| `images` / push | fails in dev | No registry was verified in `algorik-platform-dev`, and the foundation has not been applied there (no Terraform state bucket exists); expect this failure until it is |
| `infra.yml up` | cert-manager webhook "exceeded its progress deadline" | The failure ADR 0093 records (run 52). `action=diagnose` is read-only and applies nothing; `action=suspend` stops the cluster's meter |
| `infra.yml` | stuck state lock after a cancelled run | `action=force-unlock` with the lock id Terraform printed; the input is required and deliberately not defaulted |

**Rollback, once a promotion exists.** `git revert <promotion sha>` on the
default branch (or re-promote earlier freight in Kargo); Argo CD reconciles
`dev` on its own and the post-sync hook proves the serving revision carries the
reverted digest ([runbook 6.2](../RUNBOOK.md#62-rolling-back-a-deploy)). **Not
exercised**: there has never been a promotion to revert. Record the revert
commit sha, the sync id and the hook's `serves` line when it first happens.

**What not to do.** Do not re-dispatch `deploy.yml` at an older commit as a
rollback, do not `gcloud run services update` by hand, and do not run `infra.yml
up` to change what serves; none of them can roll a service back (deployment-path
page). Do not delete a cloud resource without approval naming it.

## After any incident

1. Say what you ran and what you read. A check you did not run is "not run".
2. If you halted something, write down who lifted it and why, by hand, until the
   route that records it works again.
3. If a number in a runbook was wrong, fix it in the same change, with the
   command that measures it rather than the figure.
