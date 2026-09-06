# 0048 — ADR 0023 carries both the permission and the prohibition for step 3, and the tree took the permission

**Status:** *proposed*, 2026-09-06, and **deliberately deciding nothing about
the sequence.** DEC-D10 asks that
[ADR 0023](0023-real-trading-is-the-destination-and-the-opening-is-gated.md) be
reconciled with what happened. Reconciling it means choosing between two
sentences the same accepted record contains, and choosing changes a decision
rather than corrects a description. So this record establishes exactly what the
conflict is, what each way of resolving it costs, and stops there. **DEC-D10
stays open, with this argument attached.**

**Concerns:** DEC-D10 in `../plan/PROJECT-PLAN.md` ("ADR 0023 step 3 vs the
Phase 2 gate", *overtaken in practice; ADR text unreconciled*) and the same row
in `../plan/completion-plan.md:228`.

**Relates to:** [ADR 0003](0003-paper-trading-by-default.md) and
[ADR 0021](0021-the-blueprint-expects-live-capital-and-this-platform-refuses-it.md)
(neither touched), [ADR 0022](0022-the-algorik-blueprint-is-the-architecture-of-record.md)
(the blueprint whose §51.1 gate ADR 0023 quotes),
[ADR 0035](0035-one-execution-node-in-shadow-mode.md) (step 3's shadow-mode
evidence, accepted and not applied),
[ADR 0034](0034-the-first-market-data-and-prediction-sources.md) (step 1 and 2's
external blocker).

**Does not amend and cannot amend:** ADR 0023's steps 5 to 10, the
paper-trading boundary, or anything about live order submission. Nothing below
authorises a step, retires one, or moves a layer.

---

## The conflict is inside ADR 0023, not between ADR 0023 and the tree

This is the finding, and it changes what DEC-D10 is asking for. The register's
sibling row states it in passing and does not draw the consequence:

> **ADR 0023 step 3 "buildable today"** | ADR 0023 | The record is in tension
> with itself; §5 lists it for the owner | **Overtaken in practice** — the
> feasibility gate (`95a4932`) and the attribution join (`7ef6063`) were built;
> the ADR text is unchanged (D10)
> — `../plan/completion-plan.md:228`

ADR 0023 says three things about step 3, and no two of them can be applied
together without choosing.

**The permission.**

> Steps 1 to 4 touch no boundary at all and are buildable today. Steps 5 onward
> are where the platform's safety properties change, and they are deliberately
> last.
> — `0023:82-84`

**The ordering.** Step 2 is "Pass the Phase 2 gate on real data" and step 3 is
"Build the Phase 3 execution infrastructure, still paper. Full hot path,
feasibility gate, intent netting, risk aggregates, inventory reservation,
ledger with attribution, shadow mode" (`0023:89-90`), in a table headed
"Ordered by dependency".

**The prohibition**, in the record's own reversal conditions:

> **Steps taken out of order** — most of all, execution infrastructure built
> before the Phase 2 gate passes. The blueprint's strongest instruction is
> "Stop. Do not build execution infrastructure", and a platform that builds it
> anyway has spent its effort on the assumption that the edge exists rather
> than on finding out.
> — `0023:194-198`

And a fourth clause that both readings leave violated:

> **Every step requires recorded human approval naming that step before it
> begins.** The evidence column says what must be true for approval to be
> *sought*; it is never itself the authorisation.
> — `0023:78-79`

So: step 3 is simultaneously permitted ("buildable today"), ordered after a
gate that has not been attempted, named as the record's own worst reversal
condition if built early, and gated behind an approval nothing in this
repository records. That is one record disagreeing with itself in three
directions, and it is why DEC-D10 has sat as "ADR text unreconciled" rather
than being closed by an edit.

---

## What the tree has done, and what it has not

**Step 3's components exist.** Each with a register row and a commit
(`../plan/PROJECT-PLAN.md`, Phase 0–3 table):

| Step 3's list | In the tree | Row |
|---|---|---|
| feasibility gate | edge half `95a4932`, central refusal `e8daa51` | PHASE-B10, done |
| ledger with attribution | `7ef6063`, `7d79161`; fills billed rather than placements `5290bb9` | PHASE-B11, PHASE-B22, done |
| inventory reservation | mechanism `0ca4b92`, into the node's root `63e4556` | PHASE-B12, done |
| risk aggregates | exposure buckets `588335a`, cell fills into the aggregate `98bc687` | PHASE-B19, PHASE-B20, done |
| intent netting | `qip-edge`'s netting path and its `qip_edge_netting_ratio` series | — |
| full hot path | `Cell::work` and the pass at `qip-edge-node/src/pass.rs` | — |

**Step 2 has not been attempted.** PHASE-B7 ("Attempt the Phase 2 gate on real
data") is *blocked-external*, behind PHASE-B4, behind PHASE-B3, whose blocker
is a vendor's terms nobody has read (DEC-D9). The gate has not answered *no*.
It has not been asked.

**And step 3's own evidence clause is unmet**, which is the half "overtaken in
practice" overstates. Step 3 closes on "Each component with passing-and-vetoing
fixtures; **shadow mode running against the simulator and reconciling**"
(`0023:90`). Nothing runs: `execution_nodes = {}` in every environment,
ADR 0035's single shadow node is accepted and not applied, and the whole edge
plane executes only under `cargo test`. So the *building* of step 3 was
overtaken; the *evidence* step 3 was defined by was not produced. A reader who
takes "overtaken in practice" to mean step 3 is finished will be wrong in the
direction that matters, because the unproduced half is the operational half.

---

## The three readings, and what each costs

### (i) The permission governs

"Buildable today" is scoped by the column beside it — the step table's
"Touches a layer?" answers *No* for steps 1 to 4 — so the record is saying
step 3 needs no boundary approval, and the reversal condition is a statement
about *priority*: spending effort on execution infrastructure before knowing
there is an edge is the blueprint's named mistake, not a prohibited act.

**Cost.** The record's strongest sentence stops refusing anything. "Do not
build execution infrastructure" becomes advice, and the next reader who wants
to build ahead of a gate has a precedent with this record's number on it. The
approval clause at `0023:78-79` is still unsatisfied under this reading, so it
must be amended in the same breath — otherwise the platform keeps a categorical
clause everybody knows is not followed, which is more corrosive than no clause,
because steps 5 to 8 rely on exactly that clause and they are the paper-trading
layers.

### (ii) The prohibition governs

The record ordered step 3 after step 2, named building it early as what would
make it wrong, and the tree did it anyway. The honest entry is a violation.

**Cost.** It cannot be cured: nothing un-builds a feasibility gate, and nobody
would want it un-built — the components are useful, tested, and several of them
close real defects. So the register would carry a permanent finding that
changes no behaviour, which trains readers to skip findings. Worse, an
unapproved step 3 is evidence about how the approval clause behaves under
pressure, and the clause's whole value is at steps 5 to 8. A clause first
broken on a step that touches no layer is a clause with a precedent by the time
it reaches the step that touches all of them.

### (iii) Harmonise by scope, and supply the missing record

Read "buildable today" as *needs no boundary approval*, read the reversal
condition as *effort spent ahead of evidence*, and treat the actual defect as
the missing approval: work proceeded on step 3 with no recorded decision naming
step 3. The cure is not to un-build; it is to record, once, that step 3's
paper-only components were built before the Phase 2 gate, why that was
acceptable (they touch no layer, several repair defects, and the components are
what a gate attempt would need anyway), and what it does *not* license.

**Cost.** It is the reading this author finds most defensible and it is still a
decision, not a description: it resolves an accepted record's internal
contradiction by choosing which sentence is scoped narrower. It also carries
the risk (i) carries in smaller form — a record that says "built early, and
that was fine" is quotable by the next person who wants to build early, and the
next component may not be one that touches no layer.

---

## Why this record does not choose

Because choosing changes a decision. DEC-D10's own status column says the
reconciliation is "Owner's call", and every one of the three readings alters
what ADR 0023 permits from tomorrow rather than merely describing what happened
yesterday. What an agent's record can honestly do is make the choice cheap and
exact, which is what the section above is for.

**The one thing that would sharpen it, and could not be established here.**
Whether ADR 0023 was written before or after the step 3 components landed. If
the record post-dates most of them, then it described as future work what was
already built, and DEC-D10 is largely a **description** defect that can be
corrected without changing any decision. If it pre-dates them, the readings
above are the whole question. The session that wrote this record had no shell
and could not run `git log`; the command that settles it is
`git log -1 --format=%ci -- docs/adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md`
compared against the dates on PHASE-B10, B11, B12, B19, B20 and B22 in the
register. **This is named as unverified rather than assumed either way.**

---

## The annotation this record proposes, which changes no decision

Whichever reading is taken, ADR 0023's text currently reads as though step 3
were ahead of the platform. It is not. The proposed addition to
`0023-real-trading-is-the-destination-and-the-opening-is-gated.md`, to be
applied by whoever accepts this record — in the shape
[ADR 0043](0043-the-cryptography-this-platform-has-and-the-three-gaps-no-crate-closes.md)
used to propose its amendment to ADR 0002:

> **Where this record stands against the tree, 2026-09-06 (ADR 0048).** Step 1
> is unproven in a deployment. Step 2 has not been attempted and its blocker is
> outside this repository. **Step 3's components are built** — feasibility gate,
> attribution join, inventory reservation, risk aggregates, netting and the
> cell's hot path, each with a register row and a commit — and **step 3's own
> evidence clause is not met**, because nothing runs: `execution_nodes = {}` in
> every environment. Steps 4 to 10 are untouched and no approval has been
> sought for any step. Whether building step 3 ahead of step 2 was permitted by
> this record's "buildable today" or prohibited by its own reversal condition
> is DEC-D10 and is open; ADR 0048 states the readings and their costs.

That paragraph asserts only facts and points at the open question. It is
offered separately from the three readings on purpose: correcting what a record
says about the world is not the same act as deciding what it permits, and this
record is allowed to do the first and not the second.

---

## The paper-trading boundary

Untouched, and confirmed rather than assumed. Terraform's plan-time refusal of
`supervised_live`, `limited_autonomous_live` and `autonomous_live`;
`AutonomyLevel::deployable` refusing the same three at start-up in `qip-api`,
`qip-fastbrain` and `qip-deepbrain`; `qip-edge`'s `Cell` having no constructor
taking a ceiling other than paper trading; and `qip-cost-router`'s
`Determinism::Required` arm returning a type that cannot name a model rung —
all four stand exactly as ADR 0003 and ADR 0021 left them. Every reading above
concerns steps 1 to 4, all of which ADR 0023's own table marks as touching no
layer. Nothing here creates, enables or eases an order path.

## What it costs

- **An open row stays open**, and it is the second session in a row to leave it
  so. A register row that accumulates argument without resolution starts to
  read as a place arguments go to be stored.
- **Naming the internal contradiction makes it quotable.** Until now the
  tension was implicit and the tree simply proceeded. From here, anyone can
  cite `0023:82-84` against `0023:194-198` — including someone who wants to
  build ahead of a gate for a worse reason than the platform did.
- **The correction embarrasses the "overtaken in practice" summary.** The
  register's phrase is optimistic in one direction: the components exist, the
  operational evidence does not, and a reader who quoted the phrase to a
  reviewer would be overstating the platform's position.
- **This record cannot make the missing approvals appear.** Whatever is chosen,
  there is no contemporaneous record of anybody approving step 3, and a
  retrospective approval is a different artefact from the one the clause asks
  for.

## What would make this wrong

- **The Phase 2 gate being attempted and failing.** The blueprint's instruction
  then binds in its own terms — *if no: stop* — and reading (i) becomes
  untenable regardless of anybody's preference about scope.
- **A step 4 to 10 decision being taken on the strength of this record.** It
  authorises nothing and names nothing as approved. If it is ever cited in
  support of opening a layer, the citation is the defect.
- **The date evidence coming back the other way.** If ADR 0023 post-dates the
  step 3 commits, most of this record is over-argued and DEC-D10 is a
  description fix; that would be a good outcome and this record should be
  shortened to the annotation.
- **A component of step 3 being built that *does* touch a layer.** The three
  readings all rest on the step table's "No" in the boundary column. The first
  step 3 component that answers "Yes" to that column is outside every reading
  here and needs its own approval, not this argument.
- **The register closing DEC-D10 by deleting it.** The row is unresolved, not
  obsolete; removing it would leave an accepted record contradicting itself
  with nothing pointing at the contradiction.
