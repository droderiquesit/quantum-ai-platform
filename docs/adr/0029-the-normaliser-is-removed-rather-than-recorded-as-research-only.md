# 0029 — The normaliser is removed rather than recorded as research-only

**Status:** accepted — the decision is taken. The removal is **not yet applied
in the tree**; see "What has actually happened" below, which is the honest
state and must be deleted, not amended, when the code lands.

## Context

`qip-normalization` is 845 lines of source and 573 of test under
`backend/crates/services/` (`wc -l` over `src/` and `tests/`, 1,418 together).
Nothing constructs it. Every reference to it in the workspace — nine of them,
from
`grep -rn "qip.normalization\|qip_normalization" --include=*.rs --include=*.toml backend`
— is a manifest line, its own source, its own test, or a fixture in the
acceptance crate:

- `backend/Cargo.toml:40` (workspace member) and `:129` (workspace dependency);
- its own `Cargo.toml:2` and `src/lib.rs:1`;
- its own `tests/canonicalisation.rs:10,11`;
- `backend/crates/tests/qip-acceptance/Cargo.toml:32`, under
  `[dev-dependencies]`;
- `qip-acceptance/tests/truth_loop.rs:47` and `tests/performance.rs:54`;
- one string literal in `qip-acceptance/tests/architecture.rs:762`, the
  `NO_MONEY_AUTHORITY` list.

No call site exists under `apps`, `runtime`, `edge`, `libs`, `agents`, `quant`,
or any other `services/*`. Its `contract` module — `DataContract`, `check_all`,
`FieldRule` — has no caller anywhere at all except the crate's own test file
(`canonicalisation.rs:10`).

Alignment criterion 5 says nothing exists for a phase the roadmap has not
reached unless it has a consumer today. The two backlog items that would give
it one both name a file and no owner, and `docs/plan/completion-plan.md`
records both as blocked on D6 — the decision they are offered as evidence for.
Neither can be the evidence for it.

Two facts settle the argument that it should be kept because it is complete and
would be wired soon.

**It is not complete.** `Normalizer::normalise`
(`src/normalizer.rs:244`), the batch path, never reads `self.symbols` and
never reads `self.drop_unmapped`. Symbol resolution lives in a separate public
method, `resolve_symbol` (`:302`), which a caller must invoke by hand; the only
callers in the workspace are the crate's own test and `truth_loop.rs:307`.
`grep -n drop_unmapped src/ tests/` returns exactly three lines — the field
declaration (`:167`), the write in `new()` (`:174`) and the write in the
builder `dropping_unmapped` (`:228`) — and **no reader**. The field's own
comment says:

> Drop records whose symbol has no mapping. On by default: an unmapped symbol
> is an unknown instrument, and guessing produces a phantom.

That protection does not exist. `dropping_unmapped` itself has no caller
outside its own definition. This is the defect the risk domain names for
`MaxExpectedShortfall`: a control that ships in the default constructor, reads
as protection, and cannot fire.

**It is being cited as protection it never gave.** Four documents describe it
as running:

- `docs/security/threat-model.md:206-209` — "At the ingestion boundary
  `qip_normalization::contract::DataContract` asserts field presence, ranges
  and staleness, and `ScaleGuard` flags a price that moves by more than a
  configured ratio." No boundary calls either. `DataContract` has never
  executed outside the module defining it.
- `infrastructure/terraform/modules/data/NOT-PROVISIONED.md:31-35` — rests the
  argument against provisioning Dataflow partly on transformation work that
  "is implemented in `qip-normalization` and `qip-market-ingestion` and runs
  in-process". The normalisation half runs nowhere.
- `docs/architecture/current-state-audit.md:86` — scores
  "Normalisation, dedup, time sync, enrichment" as **Built**, evidence
  `qip-normalization`, which a reader takes as built into something.
- `docs/performance/budgets.md:50` — publishes 0.31 µs / 3.27 M/s for
  "Normalisation of a bar" (0.96 µs at `:66` in debug) and describes the timed
  work as "symbol mapping, unit conversion, quality stamp". The measured call
  is `Normalizer::normalise`, which does venue aliasing, unit conversion, a
  continuity check and a timestamp clamp — **no symbol mapping and no quality
  stamp**. Two of the three things the published figure claims to cover are not
  in the measured path.

A crate nothing runs is idle. A crate nothing runs that four documents cite as
a data-quality control is a false statement about the safety of the system, and
that is the harm this record exists to stop.

Wiring it was considered and rejected. The seam is real — `qip-fastbrain`'s
`src/node.rs`, between `Feed::poll` and `Platform::observe` — but putting this
code there would ship, into the production ingest path: a symbol-mapping table
nothing populates and the batch path never consults, an inert `drop_unmapped`,
a `scale_warnings` list nothing reads or refuses on, and two clamps that
`.claude/rules/domains/core-rust.md` prohibits outright — `ScaleGuard::new`
does `max_ratio.max(1.0001)` (`:116`) and `clamp_timestamp` (`:437`) rewrites a
future-stamped record instead of refusing it. Today's sources already emit
canonical venue codes, so the venue pass would be a no-op on every record any
deployed process could see. That is not wiring a control; it is deploying one
that cannot fire, plus a rule violation, to close a documentation gap.

Nothing here touches the paper-trading boundary. `qip-normalization` holds no
money, solver, execution or issuance authority — it is in `NO_MONEY_AUTHORITY`
precisely because it holds none. Terraform's plan-time refusal,
`AutonomyLevel::deployable` in the three composition roots, and the type-level
constraints in `qip-edge`'s `Cell` and `qip-cost-router`'s `Determinism` are
untouched by every edit this record calls for.

## Decision

1. **`qip-normalization` is to be deleted**, with its workspace member entry
   (`backend/Cargo.toml:40`), its workspace dependency entry (`:129`), and the
   acceptance crate's dev-dependency on it
   (`qip-acceptance/Cargo.toml:32`). `Cargo.lock` is regenerated by a build,
   not hand-edited.

2. **The four documents above are corrected in the same change**, not after it.
   The threat model says what actually guards ingestion —
   `SensedRecord::validate()` in
   `qip-market-ingestion/src/adapter.rs:98` — and says plainly that staleness
   limits, a data-quality floor and declarative range bounds are checked by
   nothing.

3. **The SENSE stage has no normaliser, and that is the recorded position, not
   an oversight.** The next audit that finds an empty normalisation box should
   read this record before re-adding the crate.

4. **What is genuinely missing is written down here so it can be rebuilt rather
   than rediscovered.** Nothing in this workspace canonicalises a venue alias,
   converts a provider's units, or checks price continuity against the last
   accepted price. `SensedRecord::validate()` covers crossed and negative
   quotes, incoherent bars, non-positive prices and out-of-range news polarity,
   publishing a `DataQualityFailure` rather than dropping the record silently —
   and nothing else. A feed that silently switched from pounds to pence would
   pass every check this platform has, because every ratio rule is
   scale-invariant. That gap is real, it is now visible, and it is no longer
   disguised by a crate nothing calls.

5. **What this record does not settle.** The blueprint that ADR 0022 makes the
   architecture of record places a Normalizer in the *execution node* — venue
   wire format to canonical event, in microseconds — in principle 5, in §5.6
   domain 26 and in §41.2's module list. The deleted crate is a *central*
   component over `SensedRecord` built from provider payloads. They are
   different components with the same word on them, and the node-side one has
   never existed in this tree. Which of the two the platform intends is an
   owner's decision. This record deletes the central one; it does not authorise
   or forbid the regional one.

## What has actually happened

Recorded here rather than implied, because a decision record that reads as
applied when it is not is the same class of false statement this record was
written to remove.

**Applied:** this record, its index entry in `docs/adr/README.md`, and the A5
and D6 rows in `docs/plan/completion-plan.md`.

**Not applied:** the deletion itself and every document correction under
decision 2. The crate is still on disk, still a workspace member, and the four
documents still describe it as running. The removal cannot be made piecemeal:
deleting the directory while `backend/Cargo.toml:40` still names it as a
workspace member makes `cargo` refuse to load the workspace at all, so the
delete and the four manifest and test-file edits are one atomic change. The
files that change with it, none of which are this record's to touch, are
`backend/Cargo.toml`, `backend/Cargo.lock`,
`qip-acceptance/Cargo.toml`, and the three acceptance suites
`architecture.rs`, `truth_loop.rs` and `performance.rs`, plus the four
documents named in the context above and the registers listed in the
completion plan's A5 row.

One consequence is worth naming because the obvious repair is the wrong one.
`architecture.rs:795` asserts `services.len() >= 25` against exactly 25 service
crates (`ls -d backend/crates/services/*/ | wc -l` → 25), so removing a member
fails that test on its *premise*. Lowering the floor to 24 would be lowering a
bar to obtain a pass, which `.claude/rules/02-change-management.md` forbids,
and it would be wrong again on the next removal. The floor should be replaced
by an equality between what `cargo metadata` reports and what the services
directory holds — a count that tracks the directory cannot be quietly lowered,
and it catches the case the floor never could: a crate on disk that nobody
added to `members`.

## What it costs

- **Real capability leaves and it is not duplicated.** Thirteen venue aliases,
  the pence-to-pounds conversion in `Decimal`, the price-continuity guard, and
  the whole `DataContract` layer — staleness limits, a quality floor, numeric
  range bounds — exist nowhere else. This is a deletion, not a de-duplication,
  and the platform is measurably less able to detect a unit error the day after
  it lands than the day before. Decision 4 exists because of this.
- **Two paid-for defect fixes go with it**, both recorded here so the next
  implementation does not re-derive them. `PENCE_PER_POUND` is built with
  `Decimal::from_raw` rather than parsed (`normalizer.rs:42-44`), because a
  parse failure's only fallback is `ONE`, and that makes every notional a
  hundred times too large invisibly. And `clamp_timestamp` returns whether it
  changed anything (`:437`), because the caller used to count a correction on
  record kinds it never touched — a future-dated `Bar` passed through unclamped
  while being reported as fixed.
- **A flagship acceptance test is edited.** `truth_loop.rs`'s fourth stage
  becomes an explicitly test-owned value rewrite. No production code is lost
  from it, because the test already constructs and configures the normalizer
  itself (`:285-309`), but the seven-stage walk then says out loud that stage
  four is the test's and not the platform's.
- **Two published performance figures are withdrawn.** The budget check at
  `performance.rs:686-688` is one-directional — every measured stage must have a
  row, never that every row has a measurement — so it would stay green while
  `budgets.md` published a figure for a stage nothing measures. It should be
  made bidirectional in the same change, or the rot being removed here simply
  reappears in the document.

## What would make this wrong

- **Someone builds the ingest path this was meant for and has to write venue
  aliasing, unit conversion and continuity checking from nothing.** Then the
  cost in decision 4 was underpriced, and this record is the place they should
  find out what the deleted code knew.
- **A provider unit change reaches a decision undetected.** That is the failure
  the crate was shaped against; nothing catches it afterwards, this record says
  so, and if it happens the answer is to build the control on the path rather
  than to keep an unwired one beside it.
- **The blueprint's node-side Normalizer is built and it turns out the central
  one was the right home after all.** Then the decision was about the wrong
  component and should be reopened against §41.2, not against emptiness.
- **A second crate is deleted for the same reason within a phase.** One removal
  is a decision. A run of them means the tree is accumulating scaffolding
  faster than criterion 5 is catching it, and the gate needs fixing rather than
  the crates.
