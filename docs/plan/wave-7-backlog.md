# Wave 7 backlog — the next unblocked slices

Companion to `completion-plan.md` and `gap-matrix.md`, scored against the tree
at `2e19a4c` (`git log --format=%h -1`). Each item names the file or crate,
the row in the other two documents it closes, and how it would be verified.
Ordered by dependency, not by size. Nothing here is invented: every claim
below was checked against the tree in this session, not carried forward from
an earlier scoring.

---

## 1. Redeploy and observe the two blocked central services

**Closes:** completion-plan.md §6 (the open half), B2, B17, gap-matrix.md
item 4. **Not new code** — the fix already landed at `32b344d` and `2e19a4c`.

What is open: `qip-dev-api` and `qip-dev-deepbrain` have not been confirmed
running since the health-listener bind was corrected. `qip-dev-fastbrain` is
confirmed (`06bedce`); the other two failed their first revision on the same
cause, and Terraform's `deletion_protection = true` means a failed first
revision cannot be recreated by a further apply without the untaint step
`06bedce` added.

**How it would be verified:** dispatch `infra.yml` with `action=up` against
`dev`, then `deploy.yml`, and quote the run's own output — a `gcloud run
services describe` (or the pipeline's own routed-revision helper from
`da4b85e`) showing both services `Ready` and running the attested digest.
This is not code review; it is reading a real run's log, the same way
`06bedce`, `8194b3b` and `32b344d` are cited in §6.

---

## 2. A vendor host in the egress allowlist (D9)

**Closes:** gap-matrix.md item 4 (the vendor half), item 6, B3, D9;
completion-plan.md's remaining Phase-1-exit blocker.

`infrastructure/egress/envoy.yaml:392-492` names five Google/IBM clusters and
no market-data vendor. The sidecar itself is now proven plannable and, for
one service, running (§6) — the remaining gap is a decision (which vendor,
what licensing posture) and then one cluster added to the bootstrap and one
host added to `egress_allowed_upstreams`. This is an owner decision plus a
one-cluster Terraform change, not new plumbing.

**How it would be verified:** a `qip-data-finder` licensing-posture record for
the chosen vendor; the cluster and allowlist entry; a request through the
allowlist observed in the egress sidecar's access log on a subsequent apply
(the risk-register row this session left open in gap-matrix.md's "A control
that cannot fire").

---

## 3. `terraform validate` quoted by name — DONE

**Closes:** gap-matrix.md item 4 phrasing, completion-plan.md B17, §6's own
"still genuinely open" list.

Done on 2026-09-04. A `terraform` binary is present at
`/usr/local/bin/terraform` (v1.9.8). From `infrastructure/terraform`:

- `terraform init -backend=false` — "Terraform has been successfully
  initialized!"
- `terraform fmt -check -recursive` — exit 0, no output (nothing to
  reformat).
- `terraform validate` — "Success! The configuration is valid."

None of the three calls a Google API, so this closes the phrasing gap (a
named `terraform validate` run, quoted) without being evidence that anything
plans or applies cleanly against real state — that remains open per item 1
above. The narrower ask in this row's original "how it would be verified"
column — separate `validate` output for each of `cloudrun`,
`execution-node`, `trust-zones`, `egress-proxy` as standalone roots — was not
done; those are modules without their own state and `terraform validate`
does not run against a module in isolation outside a root that calls it. The
root-level run is what closes this item; a module-level equivalent, if still
wanted, is separate follow-up.

---

## 4. A producer for `feasibility_constraints` (payload slot 11)

**Closes:** completion-plan.md B10 (central half), B14 (one of the three
P3-buildable slots); gap-matrix.md's Regional Execution Mesh row indirectly.

The cell already consumes this slot (`admit_feasible` ahead of `net` in
`Cell::work`, `95a4932`) and narrows on its absence. Nothing at the centre
produces it. `qip-execution-engine` has no feasibility step of its own
(B10's still-open half) — the producer for this slot is naturally the same
work as giving the central pre-trade path a feasibility gate, so this and
B10's remainder should be done together rather than as two slices.

**How it would be verified:** `pending_policy` in `qip-api/src/mesh.rs` sets
`payload.feasibility_constraints = Slot::produced(...)`, sourced from
whatever central state names off-lot or infeasible instruments; a kernel or
API test asserting the slot ships produced and that a cell narrows less once
it applies; `grep -rn feasibility_constraints backend/crates/apps/qip-api/src
backend/crates/runtime/qip-kernel/src` no longer empty.

---

## 5. A producer for `inventory_targets` (payload slot 10)

**Closes:** completion-plan.md B14 (a second P3-buildable slot).

The central risk aggregate and the exposure buckets (`platform.rs:1144-1150`,
`B19`, done) already carry per-bucket state; `inventory_targets` is a
projection of that state into the shape the cell's `InventoryTargets` type
expects, not a new source of truth. This is lower-risk than it looks because
the source data already exists and is already tested (`risk_aggregates.rs`).

**How it would be verified:** `pending_policy` produces the slot from
`platform.risk_limits()`/the exposure buckets; a test asserting a cell
receiving it narrows one fewer capability than one that does not
(§6.2's table); mutation-verified per the standing rule.

---

## 6. The crossing-interval owner decision (D3, B13's owner half)

**Closes:** completion-plan.md D3, B13.

The code is done and spent (`153e429`): `CellConfig::crossing_interval`
takes `Passes(n)` or `Span(d)`, refused invalid, `None` by default. No root
sets one — `grep -rn with_crossing_interval backend/crates/apps` is still
empty, confirmed again this session. This is a one-line configuration
decision, not an engineering slice: what interval, in what unit, for the
execution node. Listed here because it is genuinely unblocked and cheap, not
because it is large.

**How it would be verified:** the node's composition root calls
`with_crossing_interval` with an owner-chosen value; the existing crossing
tests (`qip-edge/tests/crossing.rs`) already prove the mechanism once it is
set — no new test is needed, only the call.

---

## 7. Passkeys (B8)

**Closes:** completion-plan.md B8; gap-matrix.md's Governance & Guardrails
strong-but-incomplete row.

`grep -rln -i passkey backend/crates frontend/portal/src` is empty, confirmed
again this session — no change since the last scoring. This is Phase 0 work
with no code dependency on anything in this session's wave; it is listed here
because it remains the cheapest fully-unblocked item that touches the
frontend, and every later phase's "web and mobile" row (Phase 13) assumes
identity is already solid.

**How it would be verified:** an authenticator registration and assertion
through Identity Platform; Playwright coverage for the browser half; the grep
above no longer empty.

---

## F1–F4 — in progress this wave, not closed

`docs/ops/missing-infrastructure-register.md`'s follow-ups F1–F4 (Binary
Authorization flags on the two `gcloud run deploy` calls in
`scripts/deploy-frontends.sh`; a named service account for the landing; a
deny-egress rule for the console-egress subnet; mounting the portal's session
secret as a file instead of an environment value) are being worked by other
agents this wave. Not this document's own item — recorded here only so this
backlog does not independently claim them done or open against a stale
count. As of this note, re-reading `scripts/deploy-frontends.sh` shows
neither deploy carries `--binary-authorization` and the session secret is
still passed as `ALGORIK_SESSION_SECRET=algorik-session-secret:latest` (a
name, not a path) at `:115` — F1 and F4 are unresolved in fact as of this
reading. Do not mark any of F1–F4 done from this file; the register is the
owning record.

## What is deliberately not in this list

`compiled_plan`, `belief_priors`, `episodic_digest`, `causal_digest`,
`regime_state`, `adversary_profiles`, `trained_models` (payload slots 1–6,
12) and the per-region reservation table (B12, F6) are not here. The first
group waits on planes that do not exist yet (belief, episodic memory,
self-model — Phase 7–9 by the blueprint's own sequencing) or, for
`regime_state`, exists as a signal (`qip-cost-router/src/context.rs`) but has
no serialisation into the payload's `RegimeState` type and no consumer proven
to need it yet; building a producer ahead of a consumer is exactly the
scaffolding this repository's rules refuse. B12 is real work — a reservation
table at the cell, refusing a second proposal against one envelope, plus the
central ledger it reconciles against — and is left off this list only
because it is a clean, standalone slice already fully specified in
completion-plan.md B12's own evidence column; an owner picking up this
backlog should treat it as next after items 1–6 above, not as blocked.
