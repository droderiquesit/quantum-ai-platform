# 0108 — Any move beyond paper trading waits on named preconditions and a staged rollout, and ADR 0003 holds until the owner accepts this in writing

**Status:** *proposed*, 2026-10-06. **Not accepted. Opens nothing.**
[ADR 0003](0003-paper-trading-by-default.md) remains in force, unamended, until
the repository owner accepts this record **in writing**, in a commit or
recorded decision naming this number. Until then every sentence below that
says "would" describes a change nobody has made and nobody may make on the
strength of this text.

**Would supersede, on acceptance only:** ADR 0003, and only for the single
environment and the single stage named in "Stage 2" below. Everywhere else and
at every other stage ADR 0003 continues to hold.

**Does not supersede:** [ADR 0021](0021-the-blueprint-expects-live-capital-and-this-platform-refuses-it.md)
(capital movement, signing and withdrawal stay refused),
[ADR 0023](0023-real-trading-is-the-destination-and-the-opening-is-gated.md)
(this record is the instrument its step 5 asks for, and does not reorder its
steps), [ADR 0008](0008-edge-cells-decide-alone.md),
[ADR 0106](0106-the-ledger-refuses-any-fill-with-simulated-false-as-a-fourth-paper-trading-fence.md).

**Relates to:** [ADR 0043](0043-the-cryptography-this-platform-has-and-the-three-gaps-no-crate-closes.md)
(gap 1, asymmetric signatures), [ADR 0076](0076-a-per-person-operator-identity-is-the-consoles-asserted-subject-and-a-two-person-rule-needs-a-credential-this-platform-cannot-issue.md)
(the two-person rule needs a credential this platform cannot issue),
[ADR 0048](0048-adr-0023-carries-both-the-permission-and-the-prohibition-for-step-3.md)
(DEC-D10, still open), [ADR 0035](0035-one-execution-node-in-shadow-mode.md).

---

## Context

The owner asked for a record of the conditions under which this platform could
move beyond paper trading. ADR 0023 already fixed the destination and a
ten-step sequence, and its step 5 reads "A new ADR superseding 0003, an amended
rules file, and the owner's recorded decision. Nothing inferred". This is that
ADR, in draft. It is the step-5 artefact, offered for decision. It does not
itself take step 5.

Five defects were found in the tree on 2026-10-06. None of them shows on a
paper deployment and every one of them is a live-money failure. They are why
this record is a list of preconditions and not a plan:

1. **A default limit that could not fire.** `venue-exposure-simulated`
   (`qip-risk/src/limits.rs`, in the default limit set, located with
   `grep -n 'venue-exposure-simulated' backend/crates/libs/qip-risk/src/limits.rs`)
   read venue exposure that nothing wrote and treated the missing figure as
   zero. It is the `MaxExpectedShortfall` failure that
   `.claude/rules/domains/risk-and-execution.md` names as the pattern to avoid,
   and it happened again.
2. **The venue class is a claim made by the adapter.** `RestGateway`'s
   `Placer::is_simulated` (`qip-edge-node/src/gateway.rs`, the doc block
   headed "`is_simulated` answers `true`, and that is not a claim about the
   money") reads `AdapterClass::is_paper`, and there is no
   `AdapterClass::Live`. A sandbox adapter pointed at a production host
   therefore sends real orders and reports them as simulated. Two fences
   depend on that flag: the cell's `GATE_LIVE_VENUE` and the ledger's
   `PaperFill` / `simulated=false` refusal (ADR 0106). Both would wave a real
   fill through as paper. The type's own documentation says nothing in the
   process can check the endpoint, so this is a design gap and not a bug in
   one line.
3. **A gateway reported success without placing.** `SimulatedGateway::try_place`
   returned `Ok` for an order it never placed. On a simulator that produces a
   silent difference between what the cell believes and what the venue holds.
   On a real venue the same failure is an unhedged position.
4. **Unreviewed code reaches the integration branch.** Commits landed by
   direct push, and `main` is recorded as unprotected (CICD-045, cited in
   `docs/blueprint/assessment/CICD-b1.json` and `CICD-b3.json`). The tree has
   no `CODEOWNERS` file. Every in-code fence in this record is only as strong
   as the review that stops someone deleting it.
5. **No cell-local daily loss cap.** The centre's default set carries
   `daily-loss` (`LimitKind::MaxDailyLoss`). The cell refuses once
   `realised_loss` reaches the grant's `loss_limit` (`CapitalEnvelope::admit`,
   `qip-edge/tests/drawdown.rs`), but that bound covers the life of a grant and
   the centre issues it. ADR 0008's whole premise is a cell that keeps trading
   when it cannot reach the centre. No calendar-day realised-loss cap is
   enforced at the cell independently of the centre.

Three further facts constrain the design and were checked, not assumed:

- **`AutonomyController::request_change` changes the autonomy level. It does
  not approve orders** (`qip-risk-engine/src/autonomy.rs`). It refuses a level
  above the deployment ceiling, needs a reason, a credential younger than its
  maximum age, and a second approver for a move into a live level. ADR 0023
  requires it to keep refusing to raise the ceiling. So "every order approved
  through `request_change`" cannot be built as worded. What Stage 2 needs is
  set out in P9.
- **The second approver cannot be made honest within the current dependency
  rule.** ADR 0076 (A3) shows that two assertions minted from one shared
  Secret Manager slot are a one-person rule that reads as a two-person rule.
  ADR 0003's own third control (a second approver) therefore rests on ADR
  0043's gap 1, which is open.
- **There are four fences, not three.** The rules file names Terraform, the
  composition roots and the type system. ADR 0106 added the ledger's refusal of
  `simulated=false` fills as a fourth. Any opening has to say what happens to
  all four.

---

## Decision (proposed)

**Live order submission may be opened only in the stages below. Each stage
starts only after every precondition listed for it has been met with the
evidence named, and only with the owner's written approval naming that stage.
Evidence earns the request for approval. It never is the approval.**

ADR 0023's ordering still binds. Its steps 1 to 4 (a live market source proven
in a deployment, the Phase 2 gate passed on real data, step 3's shadow evidence,
a first venue chosen) come before Stage 2. Stage 1 below is compatible with
ADR 0023's step 3, because a provider sandbox is a paper target under
`.claude/rules/domains/risk-and-execution.md`. If the owner means to accept this
record before steps 1 to 4 close, that is a reordering of ADR 0023 and has to
be said in the acceptance, not inferred from it.

### Preconditions — code (each needs a mutation-verified test driving production code)

| # | Precondition | Evidence required |
|---|---|---|
| P1 | **Every limit in the live limit set is proven to fire.** A limit whose input is absent refuses. It is never read as zero. | For each limit in the set Stage 2 deploys, a test that drives the production path (`Platform` or `Cell::work`, not the limit's own `evaluate`) into breach and asserts the veto. A second test removes the input and asserts a refusal. A set-level test enumerates the live set and fails when a limit has no firing test. The mutation report covers each limit. `venue-exposure-simulated` either gets a writer for its input or is removed from any set that can reach Stage 2. |
| P2 | **The venue class is derived from the endpoint and the provenance of the credentials. The adapter type does not decide it.** | A closed, committed allow-list maps each sandbox host to `Sandbox`. Any host not on the list, including an unparseable one, is `Live`. A credential's class is the class of the Secret Manager slot it was read from (`qip_core::secret`), and a sandbox-classed credential on a non-sandbox host is refused at construction. Tests cover: a sandbox adapter pointed at a production host classifies as `Live` and is refused by `GATE_LIVE_VENUE`. A `Live`-classed fill is refused by the ledger fence. A host on the list with a production-classed credential is refused. Each is mutation-verified by deleting the derivation and watching the test fail. |
| P3 | **End-to-end reconciliation: what the venue says filled against what the cell recorded, and a break halts the cell.** | A test drives a placement the venue never acknowledges, then a fill the cell never sent, then a quantity mismatch. Each raises a reconciliation break, `qip_edge_halted{source}` goes to halted, and no further order leaves the cell until a human clears it. A `try_place` that returns `Ok` without placing is caught by reconciliation within one pass, not by a later audit. Reconciliation reads the venue's independent drop-copy channel, never the order-entry acknowledgement alone. |
| P4 | **A calendar-day realised-loss kill-switch in the cell, independent of the centre.** | A per-cell daily cap set at the composition root (`qip-edge-node`) from configuration and bounded at plan time. It latches a halt that only an authenticated operator clears, never at the day boundary, and keeps working while the cell is partitioned from the centre. Tests: breach while disconnected halts. The halt survives the UTC day rollover. A missing cap refuses start-up. Mutation of the comparison fails the test. |
| P5 | **Determinism is untouched.** `qip-cost-router`'s `Determinism::Required` arm keeps returning a type that cannot name a model rung. | No change. The existing tests are re-run and quoted in the request for each stage. |

### Preconditions — delivery and governance

| # | Precondition | Evidence required |
|---|---|---|
| P6 | **Branch protection on `main` and the integration branch:** required reviews, required status checks (`ci`), no direct push, no force push, admins included. | The GitHub API's branch-protection response for each branch, quoted. CICD-045 closed in `docs/DELIVERY-STATUS.md`. |
| P7 | **CODEOWNERS on execution, risk and capital paths**, at least `backend/crates/edge/**`, `qip-risk*`, `qip-execution-engine`, `qip-capital`, `qip-brokers`, `qip-portfolio`, `qip-routing`, `qip-compliance`, `apps/qip-edge-node`, `apps/qip-ledgerd`, `infrastructure/terraform/variables.tf` and the execution-node module, with code-owner review required by P6. | The committed file, and a test PR touching one of those paths that shows the required review. |
| P8 | **Binary Authorization enforced on every image that can reach Stage 2,** with attestation by `deploy.yml` and no break-glass policy in that environment. | The policy as planned and as read back from the project. A deploy of an unattested digest that is refused. |
| P9 | **A per-order human approval record**, separate from `request_change`. Each Stage 2 order carries an approval bound to a digest of its material terms (venue, instrument, side, quantity, limit price, expiry). It is produced by an operator whose credential the platform cannot itself mint (ADR 0043 gap 1, ADR 0076 A3), and the cell verifies it before `Cell::send`. | Its own ADR, because closing gap 1 needs either an external identity terminator or a verification dependency under ADR 0012. Tests: an order without an approval, with an approval for different terms, or with a replayed or expired approval is refused at the send seam. |
| P10 | **Observability that pages a person.** `workload_metrics_exist = true` for the Stage 2 environment, justified by evidence that the execution node's metrics were actually ingested. Alert policies on halt, reconciliation break, daily-loss latch and ledger-fence refusal route to the named on-call person. | The ingestion evidence that `.claude/rules/domains/observability.md` asks for, and one drill per policy that reached the on-call person's device. |

### Preconditions — external, owner only

Agents cannot complete these, and must not appear to. Each is evidenced by
the owner's own written statement in the acceptance record.

- **E1.** A broker account and API agreement in the owner's own name, with the
  venue's terms read for automated order entry.
- **E2.** A regulatory and compliance review for the owner's jurisdiction,
  covering automated trading on the chosen venue and asset class.
- **E3.** Venue credentials held only in Secret Manager, read under Workload
  Identity Federation, mounted as files only to the Stage 2 execution node,
  with the IAM binding absent in every other environment (ADR 0003's fourth
  control). No downloaded key, ever.
- **E4.** An incident runbook naming the human on call for the whole of
  Stage 2, with the kill-switch procedure and the venue's own cancel-all route,
  rehearsed once before the stage starts.

---

## The stages

### Stage 0 — paper trading (today)

All four fences stand exactly as they are. Nothing in this record changes
Stage 0.

**Exit criteria to request Stage 1:** P1 to P5 merged with tests and mutation
reports. P6 to P8 in place. ADR 0035's shadow node running against the
simulator and reconciling (ADR 0023 step 3's unmet evidence clause, per
ADR 0048).

**Fence changes:** none.

### Stage 1 — provider sandbox certification

The cell sends to a provider's sandbox through `RestGateway`, with the venue
class derived per P2 and classified `Sandbox`. This is a paper target, so the
autonomy ceiling stays `paper_trading`.

**Exit criteria to request Stage 2:** a continuous certification run of a
length the owner fixes in the approval. In that run: zero unexplained
reconciliation breaks. Every limit in the live set made to fire at least once
by a deliberate drill. The P4 latch drilled. The kill switch drilled. A
deliberate misconfiguration (sandbox credential plus production host) refused
at start-up. Every ledger posting reconciled to the sandbox's drop copy. Also
P9 and P10 complete, E1 to E4 stated by the owner, and ADR 0023 steps 1, 2 and 4
closed or explicitly reordered by the owner.

**Fence changes:** none to the four fences. The one change is in
`qip-edge-node`'s composition root: `QIP_VENUE_FEED` would admit a
`sandbox` value next to `simulated`, and only when P2's derivation agrees.
Any other value still stops the process naming ADR 0003.

### Stage 2 — supervised live, capped and time-boxed

One environment, one execution node, one venue, one asset class, the
`supervised_live` level only. Every order carries a P9 approval. The daily
capital at risk is capped at a figure the owner writes into the acceptance;
this record does not choose it. **The stage has a fixed end, proposed as 30
calendar days from its start.** At that instant every fence falls back to
paper by itself, with no action needed. Continuing needs a further ADR.

**Exit criteria (the stage ends, whichever comes first):**
- the expiry instant passes, and the platform reverts to paper on its own;
- any unexplained reconciliation break, any ledger-fence refusal, or any P4
  latch firing twice ends the stage, and the platform returns to paper until
  a written review is done;
- the owner withdraws approval.

A good result earns only a request for a Stage 3 ADR.

**Fence changes that would be needed (described here, not made):**

1. **Terraform, `infrastructure/terraform/variables.tf`.** The second
   `validation` block on `autonomy_ceiling` would stop being a blanket
   refusal:
   - It still refuses `limited_autonomous_live` and `autonomous_live` in
     every environment.
   - It admits `supervised_live` only when three things hold. The environment
     is the one named in the acceptance. A new `live_stage_expires_at` variable
     is set and later than `plantimestamp()`. A new `live_daily_capital_cap` is
     set and at or below a ceiling held in the root module.
   - `paper-boundary.tftest.hcl` would gain one plan per rung per environment,
     with the admitting half and both refusing halves (ADR 0069). The
     `live_capable` output would be true only there.
   - The venue credential's IAM binding would exist only where `live_capable`
     is true.
   - Because the plan-time check sees time only at plan, an expired stage
     still needs layer 2 to refuse it at start-up.
2. **Composition roots, `AutonomyLevel::deployable`.**
   - It would take the expiry instant and the daily cap alongside the
     configured level.
   - It admits `SupervisedLive` only when both are present, the expiry is in
     the future and the cap is positive. It refuses the other two live levels
     unconditionally.
   - Absent configuration still returns `PaperTrading`.
   - A process running past the expiry halts its execution scope through the
     kill switch. It does not lower itself silently, because a process that
     quietly lowers itself keeps an operator believing it is live.
   - `qip-fastbrain` and `qip-deepbrain` send no orders and would keep the
     present refusal. Only `qip-api` and `qip-edge-node` (which builds the
     cell's controller) would change.
3. **Type system, `qip-edge`'s `Cell`.**
   - `Cell::new` stays paper-only. A *separate* constructor would take a
     `SupervisedLiveAuthorisation`. Only the composition root can construct
     that value, from the verified expiry, cap and P2 venue class, and it
     carries all three. This is what ADR 0023 step 8 asks for: a second
     reviewed act, not a parameter.
   - `GATE_LIVE_VENUE` stays unconditional for any cell built by `Cell::new`.
     It admits a `Live`-classed venue only on a cell holding the
     authorisation, only before its expiry, and only with a P9 approval for
     those exact terms.
   - `Determinism::Required` in `qip-cost-router` **does not change** (P5,
     ADR 0023).
4. **The fourth fence, ADR 0106, and `PaperFill` in `qip-portfolio`.** The
   refusal of `simulated=false` would not be flipped. A live fill would get its
   own record kind, posted to a ledger partition separate from paper. The
   existing refusal keeps rejecting an unmarked or misclassified fill, so a
   live fill can only reach the balance sheet through the path built for it.
5. **`AutonomyController::request_change`** is unchanged. Inside the raised
   ceiling it is the one route from `PaperTrading` to `SupervisedLive`. It
   keeps its two-approver rule and keeps refusing to raise the ceiling. With
   P9 in place its second approver becomes honest.
6. **Rules.** `.claude/rules/01-security-and-safety.md` would be amended in the
   same acceptance to name the Stage 2 exception, its environment and its
   expiry. No agent may make that amendment except as the owner's recorded
   act.

### Later stages

Only by a further ADR, written against Stage 2's evidence. Nothing here
authorises `limited_autonomous_live`, `autonomous_live`, a second venue, a
second environment, or any movement of capital (ADR 0021, ADR 0023 step 10).

---

## Alternatives rejected

- **A single switch (one flag, variable or ConfigMap key enabling live).**
  Rejected. It turns three independent fences into one value, and ADR 0003
  exists so that a forgotten or mistyped value leaves the platform *safer*.
- **A ceiling parameter on `Cell::new`.** Rejected, for ADR 0023 step 8's
  reason. The absence of a constructor is what makes today's guarantee
  structural, and a parameter replaces a reviewed act with an argument.
- **Going straight to `limited_autonomous_live` with a small cap.** Rejected.
  The first contact with a real venue is where P2 and P3 failures surface, and
  a human approving each order is the only control that sees the order before
  the venue does.
- **Using `request_change` for per-order approval.** Rejected. It moves a
  level, and stretching it over orders would mean either raising the ceiling
  at runtime (forbidden by ADR 0023) or an approval not tied to an order's
  terms, which approves nothing in particular.
- **Superseding ADR 0003 now and tracking the preconditions as work items.**
  Rejected. Acceptance would then be the act and the preconditions would be
  advice, the way the inverted reading of ADR 0023 turned out in ADR 0048.
- **An open-ended Stage 2.** Rejected. An approval with no end becomes
  standing permission, and making the expiry fail closed means the stage
  needs a decision to continue, not a decision to stop.

---

## Dependency direction

This record changes no code and adds no crate. The changes it describes keep
every arrow pointing inward:

- **P2's venue classification.** The allow-list type and the
  derivation are pure logic in `qip-routing` / `qip-brokers` (edge and
  service). The host and the credential slot are read in `qip-edge-node`, an
  app, and passed in. No lib reads configuration, and no service reads the
  environment.
- **P4 and the Stage 2 constructor.** These live in `qip-edge`, which depends on
  libs and a subset of services. The authorisation type it consumes is
  constructed in the app. The cell never depends on the runtime or on an app.
- **P9's approval record.** It is a data type in `qip-contracts` (lib),
  verified in the cell and checked in `qip-risk-engine` (service), and
  composed in `qip-kernel` (runtime). No service depends on the runtime.
- **The `deployable` change.** It stays in `qip-risk-engine` and is called
  from the apps, as today.
- **P9's verifier.** It may need a third-party crate or an external
  terminator. Either needs its own ADR under ADR 0002, 0009 and 0012, and this
  record approves neither.
- **No async runtime** is proposed or needed. Venue I/O stays blocking with
  explicit timeouts.

---

## What it costs

- **Time and friction, on purpose.** Ten preconditions, four owner-only
  prerequisites, two stages before any real order, and a stage that ends on
  its own. Someone in a hurry will see this as obstruction. That is the
  pressure ADR 0023's "What it costs" warned about, and the reply is the same:
  the gates are in Terraform, four fences and a test suite, not in this prose.
- **A record like this makes the boundary look negotiable.** Writing down how
  to open it lowers how strongly people defend it. The mitigation is that
  acceptance is a single written act by one person, and every stage has an
  expiry and an automatic return to paper.
- **P9 probably needs the first dependency beyond `serde` since ADR 0013,**
  or infrastructure outside the process. Either way it adds audit and supply
  chain surface that this platform has so far refused to carry.
- **A per-order human approval removes ADR 0008's latency advantage for the
  whole of Stage 2.** Stage 2 therefore says nothing about whether a strategy
  sensitive to latency works live. It proves plumbing and nothing more, and
  results from it must be quoted that way.
- **Operational load on one person.** E4 names a human on call for every hour
  the stage runs, and the platform's only operator is that person.
- **Real money at risk,** bounded by the owner's daily cap, P4's latch and
  the stage's expiry. It is not zero, and nothing in this record makes it zero.

## What would make this wrong

- **Any stage beginning without the owner's written approval naming it,** or
  this record being cited as approval while its status is *proposed*. Either
  means it was read as permission, which is the failure it is written against.
- **A precondition met by assertion and not by evidence.** That includes a
  limit "known" to fire with no driving test, a reconciliation "known" to halt
  with no break injected, or branch protection described but not read back
  from the API. It would rebuild `MaxExpectedShortfall` and
  `venue-exposure-simulated` one layer up.
- **A fence opened by parameter, flag or test shortcut,** not by the separate
  reviewed acts described above, and most of all a `simulated=false` refusal
  simply flipped.
- **Stage 2 continuing past its expiry,** or after an unexplained break, on
  the ground that it was going well.
- **P9 delivered on a shared secret.** A two-person or per-order approval that
  one slot can forge is a control that reads as protection and is not
  (ADR 0076).
- **The owner deciding the platform stays paper permanently.** Then this
  record should be rejected, not left proposed where someone might take it
  up again, and ADR 0023 superseded with it.
- **ADR 0023's ordering being skipped silently.** If Stage 2 is requested
  before a live market source and the Phase 2 gate, the owner has to say so
  in the acceptance. Otherwise the request is premature.

## The paper-trading boundary

Untouched by this record, and checked rather than assumed on 2026-10-06:

- `variables.tf`'s second validation on `autonomy_ceiling` refuses all three
  live rungs.
- `AutonomyLevel::deployable` returns `PaperTrading` on absent configuration
  and refuses every live level.
- `Cell::new` has no constructor taking a non-paper ceiling.
- `Determinism::Required` returns `DeterministicRouting`.
- ADR 0106's ledger fence stands.

This file changes no code, no Terraform, no workflow and no rules file, and
creates, enables or eases no order path.
