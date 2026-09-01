# 0023 — Real trading is the destination, and the opening is gated

**Status:** accepted — records intent and a sequence. **Opens nothing.**
**Does not supersede:** ADR 0003 or ADR 0021. Both remain in force. See
"What this does not do", which is the most important section of this record.

## Context

The owner has decided the product destination: **real trading is the intended
end state. Paper trading is the correctness harness on the way there, not the
end state.**

That is consistent with the architecture of record rather than in tension with
it. Blueprint §1.3 is explicit that two hundred dollars is "a correctness
harness, not an engine — real money at risk to prove the plumbing, which is
exactly what the Phase 3 gate exists for". The blueprint has always described a
platform that trades real capital; ADR 0022 made it the architecture of record;
this record states that the destination is accepted rather than refused.

What has not changed is the ordering, and the ordering is the whole of this
record. The blueprint puts a gate in front of execution infrastructure and
words it more strongly than any other gate in the document (§51.1):

> **End of Phase 2** — Does a family survive holdout with honest significance
> after cumulative trial correction? If no: **Stop. Do not build execution
> infrastructure. This is the most important gate in the document.**

And Phase 3 is the live-money gate: thirty days live, inside the holdout band.

**Zero of the four gates have passed.** Not one. The traceability matrix
records why: each gate is an empirical claim about real data or a real venue,
and every deployment's data is synthetic or replayed. So the authoritative
design itself defers live execution, and this record changes intent without
disturbing that sequence.

## Decision

Real trading is the destination. The opening is sequenced, gated, and
**unexecuted**. The sequence is in "The opening sequence" below.

C1 in the traceability matrix changes status accordingly. It was "the
authoritative design specifies something this platform deliberately refuses".
It is now **"the destination is agreed; the opening is gated and unexecuted"**.
It is no longer a conflict. It is sequenced work with a hard precondition.

## What this does not do

**This record opens nothing, authorises nothing, and permits no step toward
live trading to begin.** A future reader who takes "real trading is the
destination" as licence to build toward it has misread it, and this section
exists because that misreading is the predictable one.

Specifically:

- **ADR 0003 stands and is not superseded.** Paper trading remains the default
  and the ceiling. It is superseded only by step 6 below, explicitly, with
  recorded approval — not by this record and not by implication from it.
- **ADR 0021 stands exactly as written.** The deterministic half may be built;
  signing and withdrawal may not.
- **All three paper-trading layers stay intact**, and no step below touches one
  until its own row has been approved.
- **`.claude/rules/01-security-and-safety.md` still governs**, and it says the
  boundary cannot be weakened by a task instruction. A destination is not an
  instruction to arrive, and an ADR recording intent is not an amendment to a
  rules file. Step 6 is that amendment and it has not happened.
- **`no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
  stays**, along with every other enforcing test.
- **No live venue is contacted, no credential provisioned, no ceiling raised.**

The distinction this record depends on is the one ADR 0020 and ADR 0022 also
depend on: **direction is not authorisation, and evidence earns a conversation
rather than a machine.** An agent that finds itself reasoning "the destination
is live trading, therefore this live-adjacent change is in scope" has
constructed exactly the inference all three records forbid.

## The opening sequence

Ordered by dependency. **Every step requires recorded human approval naming
that step before it begins.** The evidence column says what must be true for
approval to be *sought*; it is never itself the authorisation.

Steps 1 to 4 touch no boundary at all and are buildable today. Steps 5 onward
are where the platform's safety properties change, and they are deliberately
last.

| # | Step | Touches a layer? | Evidence that closes it | Blueprint phase |
|---|---|---|---|---|
| 1 | **Prove one live market source.** The wiring exists — `feed.rs:61` declares `Live(Box<RestMarketDataAdapter>)`, `:108` constructs it behind the licensing gate. What is missing is evidence that a deployment absorbs real data through it | No | A cycle observed absorbing live data behind the licensing gate, in a deployment, with the licensing posture recorded | 1 |
| 2 | **Pass the Phase 2 gate on real data.** The machinery exists: `qip-simulation-engine/src/validation.rs`, `qip-lifecycle/src/gates.rs`, `qip-lifecycle/src/evidence.rs` | No | A family surviving holdout with honest significance **after cumulative trial correction**, on data from step 1 rather than synthetic or replayed. The trial count must be cumulative across every candidate ever evaluated, not per-run | 2 gate |
| 3 | **Build the Phase 3 execution infrastructure, still paper.** Full hot path, feasibility gate, intent netting, risk aggregates, inventory reservation, ledger with attribution, shadow mode | No | Each component with passing-and-vetoing fixtures; shadow mode running against the simulator and reconciling | 3 |
| 4 | **Choose the first venue, narrowly.** The blueprint reorders itself around prediction markets (Phase 6) precisely because that is the one arena where small capital is an advantage rather than a handicap | No | A named venue, its adapter promoted through sim, its licensing posture evaluated, and the argument for why it is first | 6 |
| 5 | **Supersede the rules and ADR 0003 explicitly.** `.claude/rules/01-security-and-safety.md` and ADR 0003 must be superseded in writing, not quietly contradicted | This is the amendment itself | A new ADR superseding 0003, an amended rules file, and the owner's recorded decision. Nothing inferred | — |
| 6 | **Open Terraform's ceiling for one environment.** `infrastructure/terraform/variables.tf:105-116` refuses `supervised_live`, `limited_autonomous_live` and `autonomous_live` at plan time | Layer 1 | A plan proving the gate admits the intended level for the intended environment **and still refuses the other two, and still refuses all three elsewhere** | — |
| 7 | **Open the composition roots for that deployment.** `AutonomyLevel::deployable` at `qip-api/src/main.rs:60`, `qip-fastbrain/src/main.rs:114`, `qip-deepbrain/src/main.rs:138` | Layer 2 | Tests proving it admits the approved level and refuses the rest; and that absent configuration still defaults to paper, never to live | — |
| 8 | **Assemble a live-capable cell deliberately.** `Cell::new` (`qip-edge/src/cell.rs:148`) has no constructor taking a non-paper ceiling | Layer 3 | A separate, reviewed constructor — not a parameter added to the existing one — so that assembling a live cell stays a distinct act | — |
| 9 | **Run inside the Phase 3 gate.** Thirty days live, inside the holdout band | — | Thirty days, performance inside the band, no unexplained break. If it fails, the gap is the finding and live trading stops | 3 gate |
| 10 | **Capital movement, separately and later.** Corridors, transfer gate, custody policy, destination registry, reconciliation, approval delays | A different boundary | Per ADR 0021, which this record does not supersede: a separate decision, separately approved | 12 |

### Why step 10 is separate, and last

**Order submission and capital movement are two boundaries, not one.** A
platform can trade live while capital movement stays closed, and it probably
should, first. Trading live with capital that can only be moved by a human is
a materially smaller blast radius than trading live with autonomous transfer
corridors, and the two failures are different: a bad order loses a position,
while a bad transfer loses the capital. ADR 0021's refusal of signing and
withdrawal is not weakened by this record and is not on the critical path to
step 9.

### "Live" need not mean "live everywhere at once"

Step 4 is in the sequence because the blueprint's own ordering argues for it.
Version 8 optimised for microseconds; at this capital level the binding
constraint is information and the fee-to-edge ratio, and §1.3 names four
arenas where small capital is an advantage — prediction markets, maker rebates
on quiet pairs, funding capture, and small-notional inefficiencies. A first
opening that is one venue, one asset class and one strategy family is both the
smallest blast radius and the arena the design says is most likely to work.

## The four boundary points, and which should stay closed

The three layers are named individually below, along with a fourth control
that is often counted with them and should not be opened at all.

| # | Control | Where | What opening it would mean |
|---|---|---|---|
| 1 | Terraform plan-time refusal | `variables.tf:105-116` | Admit one live level for one named environment. It must continue to refuse the others, and to refuse all three in every other environment. This catches the reviewed, committed mistake |
| 2 | `AutonomyLevel::deployable` | `autonomy.rs:110`, called at all three composition roots | Admit the approved level at start-up. Absent configuration must still default to `PaperTrading` — a deployment that says nothing must never get live. This catches the unreviewed `kubectl edit` that Terraform never sees |
| 3 | `Cell::new` | `qip-edge/src/cell.rs:148` | A *separate* constructor for a live-capable cell, so the absence that makes today's claim structural is replaced by a second reviewed act rather than by a parameter |

**Two controls that should stay closed even after live trading opens:**

- **`AutonomyController::request_change` must keep refusing to raise the
  ceiling.** `autonomy.rs:572-577` returns "this deployment's ceiling is {},
  so {} cannot be reached; raising the ceiling is a deployment change, not a
  runtime one". That property is worth keeping regardless of destination: it
  means a running process cannot be talked upward by an operator, a config
  reload or a compromised control path. Moving *within* the ceiling with two
  authenticated approvers is the intended mechanism and is unaffected.
- **`qip-cost-router`'s `Determinism::Required` arm must stay as it is.**
  `router.rs:400-406` returns a type that cannot name a model rung, so a
  deterministic pre-trade check cannot be routed to a model. This has nothing
  to do with paper versus live — it is the rule that risk checks are
  deterministic — and going live makes it more important, not less.

## What could not be specified

Stated plainly rather than invented, because a criterion that cannot be met is
worse than an acknowledged gap:

- **The significance threshold and the cumulative trial count for step 2.**
  The machinery exists but the repository does not record how many candidates
  have ever been evaluated, and cumulative trial correction needs that number.
  Until something counts trials across runs, "honest significance" cannot be
  computed, only claimed. This may be a gap in the tree rather than in the plan.
- **The holdout band for step 9.** "Inside its holdout band" requires a band,
  and no band is defined anywhere in the tree. It has to come from step 2's
  output and does not exist yet.
- **Which environment opens first, and the capital at risk.** Both are owner
  decisions and neither is inferable from the repository.
- **What "thirty days live" means for a venue that does not trade continuously.**
  Prediction markets resolve on events, not on a clock, so the Phase 3 gate's
  duration may need restating for the venue step 4 chooses. Noted rather than
  resolved.
- **Whether the frontends must reach Leptos before live.** ADR 0022 makes
  Leptos the target; nothing establishes it as a precondition for trading, and
  asserting either way would be invention.

## What it costs

Recording a destination creates a standing pressure toward it, and the whole
of this document is an attempt to make that pressure land on a conversation
rather than on a commit. That attempt has a cost: ten gated steps and a
lengthy prohibition section are friction, and friction is what gets removed
when somebody is in a hurry. The mitigation is that the gates are enforced by
Terraform, three composition roots, a type system and a test suite rather than
by this document — but the sequencing and the ordering live only here.

It also puts a genuine deadline on work that had none. Item 1 of the sequence
was previously an item on a plan; it is now the first thing standing between
this platform and its stated destination, which changes how its absence reads.

The largest cost is that this record makes the paper boundary look temporary,
and a control that is believed to be temporary is defended less carefully than
one believed to be permanent. Every layer stays exactly as strong as it was;
what changes is how somebody feels about it, and that is precisely the erosion
this section exists to name.

## What would make this wrong

- **Any step beginning without recorded, step-named approval.** That would mean
  this record was read as authorisation, which is the failure it is written to
  prevent.
- **Steps taken out of order** — most of all, execution infrastructure built
  before the Phase 2 gate passes. The blueprint's strongest instruction is
  "Stop. Do not build execution infrastructure", and a platform that builds it
  anyway has spent its effort on the assumption that the edge exists rather
  than on finding out.
- **A layer weakened without its row.** Opening Terraform because a test needed
  it, or adding a ceiling parameter to `Cell::new` for convenience, are how a
  gated sequence becomes an ungated one.
- **The Phase 3 gate failing and live trading continuing.** The blueprint's own
  answer is "Stop live trading. The gap is the finding" and there is no reading
  of this record that overrides it.
- **The destination changing.** If the owner decides the platform stays paper
  permanently, this record is superseded and the sequence is deleted rather
  than left lying around as a plan somebody might resume.
