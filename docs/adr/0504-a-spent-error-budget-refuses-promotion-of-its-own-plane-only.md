# ADR 0504: A spent error budget refuses promotion of its own plane only: qip-api serves each plane's release standing read-only, the Kargo promotion out of a stage reads it, and a standing that cannot be read refuses

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation
  (decision 14, ADR_THEN_IMPLEMENT)**, session t50-39 (wave580). It
  tightens: it adds a refusal to the `paper` promotion and removes none, and
  touches no paper-trading layer. The route it adds is read-only.
- **Date**: 2026-11-13
- **Reads**: ADR 0036 (promotion happens in Kargo), ADR 0093 (the control
  plane is suspended), ADR 0003 (paper only), `docs/blueprint/lanes/wave1031-owner-rulings-2-2026-10-31.json`
  (the ruling, verbatim).
- **Rows**: R12-OBS-9156 (COMPLETE in-tree 2026-11-14; what waits outside the repository is named below).

## Context

§49.1's objectives were evaluated per plane (`blueprint_objectives.rs`) and
`qip_observability::aiops::release_gate` refused a release for a plane whose
observed, page-worthy objective had spent its budget. Nothing that promotes
anything asked it: no process served the answer, and the promotion steps in
Kargo read only the committed tree. The owner ruled that a spent budget
refuses promotion of that plane and no other, at the Kargo promotion out of
a stage, beside the paper-ceiling step, and that a standing that cannot be
read refuses.

One defect was found on the way. `release_gate` filters statuses by plane
and answers `Proceed` when none match. That is right for a declared plane
nothing has spent, and it is also the answer for a misspelled plane, so a
promotion step asking about `edges` would have read a green light for ever.

## Decision

1. **The kernel answers per plane and refuses an undeclared one.**
   `blueprint_objectives::release_standing(ledger, plane, now)` returns the
   `release_decision` for `plane` with two counts beside it: the plane's
   objectives resting on an observation and those resting on none. A plane
   §49.1 declares no objective on is an `Error::invalid` naming the planes
   that exist. The set of planes is `release_planes()`, read from
   `blueprint_slos`, so it has one owner. `Platform::release_standing`
   exposes it.
2. **qip-api serves it read-only at `GET /api/v1/release/:plane`**, role
   `Monitor`. The status is the answer, because a promotion step reads the
   status first: 200 when the plane may be promoted, 409 naming the
   objective whose budget is spent, 404 for an undeclared plane, and the
   existing 503 for a poisoned platform. The body carries
   `observed_objectives` and `unobserved_objectives`, so a 200 that rests on
   nothing is visible as that.
3. **Kargo asks once per plane its freight carries, and no other plane.**
   `PromotionTask verify-release-standing` (`kargo/base/releasetask.yaml`)
   takes `plane` and `releaseStandingURL`. It refuses at once when either is
   empty. Then it makes one `http` GET with a 10 s timeout, which succeeds
   on exactly `response.status == 200` and fails on every other status. No
   answer leaves the step undecided. The `paper` Stage runs it after
   `verify-paper-ceiling` and before `record-gate`, once for `kernel` and
   once for `portfolio`. `qip-images` carries qip-api, qip-fastbrain and
   qip-deepbrain, which is the central plane. Its §49.1 objectives are
   declared on `kernel` and `portfolio`. `edge`, `edge-node` and `strategy`
   are the cell's planes. No Kargo freight carries qip-edge-node, so a spent
   `edge` budget refuses nothing here. That is "that plane only".
4. **Unobserved is not refused.** The ruling refuses a spent budget. Most
   §49.1 objectives have no figure in this tree yet, and
   `blueprint_objectives.rs` says which ones and why. Refusing on
   "unobserved" would refuse every promotion for a reason no change here
   can clear. The count travels in the body instead.
5. **Until an address and a credential exist, every promotion out of
   `paper` is refused.** `releaseStandingURL` is empty in the Stage, and
   the Secret `qip-release-standing` the header reads does not exist. Under
   the ruling, "a standing that cannot be read refuses", this is the
   intended behaviour and not a defect. The control plane is suspended
   (ADR 0093), so nothing is stopped today that was running.

## Consequences

- `qip-api/tests/release_routes.rs` drives the netting-ratio objective
  (plane `edge`) into a spent budget through `Platform::ingest_cell_report`.
  It then asserts that `edge` answers 409 naming `netting-ratio` and that
  `kernel`, `edge-node`, `strategy` and `portfolio` each answer 200. It also
  asserts that `edges` is refused with a 404. `qip-acceptance`'s `gitops`
  test `the_paper_stage_refuses_a_spent_or_unreadable_release_standing_for_its_own_planes_only`
  pins the four Kargo properties. It checks the asked planes against
  `release_planes()`.
- The planes the freight carries are a judgement, written in two places:
  the Stage's comment and `FREIGHT_PLANES` in the gitops suite. The suite
  refuses any drift between them. A new central objective on a new plane
  needs a third invocation and an edit to that constant.

## What it costs

- Every promotion out of `paper` is refused until a qip-api address and a
  Monitor credential are configured. While the control plane is suspended
  this stops nothing that runs. Once the plane is up, it is a release that
  waits on a person.
- A third freight plane, or a renamed one, takes edits in two places: the
  Stage and `FREIGHT_PLANES`. The suite catches drift between them, but
  someone still has to make both edits.
- One more authenticated read surface on qip-api. It is read-only, and it
  answers about budgets, not positions.
- An unobserved plane still answers 200. The body says it rests on
  nothing, but the step does not refuse on that.

## What would make this wrong

- If the freight split per binary, `kernel` and `portfolio` would no longer
  be the planes of one bundle. Each would then need its own Stage chain, or
  a spent `kernel` budget would hold back a binary the objective never
  measured.
- If Kargo's `http` step left a status outside both expressions to retry,
  or if `secret()` did not resolve as written, the step would not fail
  closed the way decision 3 says. The first live promotion settles both
  questions.
- If the owner rules that "unobserved" must refuse, decision 4 would be
  reversed.

## What remains outside the in-tree grade (2026-11-14)

- **Most budgets cannot be spent yet.** Only `reconciliation-breaks-zero-unexplained`
  (`kernel`) and `netting-ratio` (`edge`) are fed, and both only from
  `ingest_cell_report`. `portfolio` has no fed objective, so its check can
  never refuse until one is fed.
- **No real promotion has read a standing.** That needs a qip-api the
  control plane can reach, its address in `releaseStandingURL`, a Monitor
  credential in the `qip-release-standing` Secret, and an unsuspended
  control plane. Kargo's `http` step and its `secret()` expression have
  been read and pinned, and no controller has evaluated them.
- **The cell's planes have no promotion to gate.** qip-edge-node is
  released by Terraform onto an execution node, and none is deployed.
  Gating that release on `edge` and `edge-node` is a separate decision.
