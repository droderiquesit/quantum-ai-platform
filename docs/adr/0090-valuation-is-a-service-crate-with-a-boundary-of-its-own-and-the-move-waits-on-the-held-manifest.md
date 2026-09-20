# ADR 0090: Valuation is a service crate with a boundary of its own, and the move waits on the held manifest

- **Status**: Accepted on the authority the owner delegated on 2026-09-19,
  recorded in ADR 0081's status line and used since by ADR 0082, 0083 and
  0084 for decisions of this shape — a workspace-layout decision taken now
  so that a lane moving files later inherits a decision and not a question.
  **Nothing is moved by this record.** The manifest it touches is held by
  another lane tonight, and §4 says what the moving lane does and in what
  order.
- **Date**: 2026-09-20
- **Supersedes**: nothing. Corrects one sentence in
  `backend/crates/runtime/qip-kernel/src/valuation.rs`'s module documentation
  (§1 says which) and one clause of the register's §1.2 row (the
  Consequences' last bullet).
- **Related**: ADR 0016 (one layout; the four-domain map this adds a crate
  under), ADR 0001 and ADR 0011 (Rust everywhere; one workspace), ADR 0002
  and ADR 0009 (two dependencies — a workspace crate is not one), ADR 0050
  (the volatility surface, the one valuation engine reached by nothing, and
  why), ADR 0008 (the hot path consults nothing it does not need), the
  boundaries rule in `.claude/rules/architecture/00-boundaries.md` (libs ←
  services ← runtime ← apps).

## Context

Blueprint §1.2 names seven planes and gives each a function and a time
scale. The register's row for it has said since 2026-09-19 that six of the
seven have a crate home and the seventh, Valuation, is "a kernel module
rather than a plane", that every function the plane's row describes exists
and is production-reached, and that what is absent is a *boundary*: "nothing
stops a future edit reaching across it." It then said the move is "an
architecture decision rather than an implementation, and it should be argued
in an ADR before a lane starts moving files." This is that record. Every
premise below was re-run in the tree it was written against, on 2026-09-20,
rather than taken from the row.

**What the module is.** `wc -l backend/crates/runtime/qip-kernel/src/valuation.rs`
prints 543, and
`grep -n '^pub \(struct\|enum\|fn\|trait\|type\|const\)' backend/crates/runtime/qip-kernel/src/valuation.rs`
prints one line: `CreditRegister`. The module is one type — the composition
of `qip-market`'s `TermStructure` (discounting) with `qip-financial`'s
`CreditProfile` (default probability, recovery, covenant state) over the
`Universe` the platform was assembled with, producing per-claim expected
losses in `Decimal` and a list of problems for the UNDERSTAND stage to
report.

**What it depends on, and what it does not.**
`grep -n '^use ' backend/crates/runtime/qip-kernel/src/valuation.rs` prints
`std::collections::BTreeMap`, `qip_core`, `qip_financial` (three paths) and
`qip_market::curve` — libraries only — and
`grep -c 'crate::\|super::' backend/crates/runtime/qip-kernel/src/valuation.rs`
prints `0`. The module reaches nothing else in the kernel. It is a service
crate's worth of code that happens to sit in the runtime's directory.

**Who reads it.**
`grep -rn 'crate::valuation\|credit_register()' backend/crates --include=*.rs | grep -v '/tests/'`
prints five lines, all in `qip-kernel`: the `pub use` in `lib.rs`, the
`credit` field on `Platform`, its construction in `Platform::new`
(`CreditRegister::from_universe(&universe, now)`), and the accessor
`credit_register()`. The two reads that matter are in `stage_understand` —
`grep -n 'self\.credit\.' backend/crates/runtime/qip-kernel/src/platform.rs`
prints `summary()` and `problems()` — which is the production call path the
register credits. Seven tests drive it, every one through `Platform`
(`grep -c '#\[test\]' backend/crates/runtime/qip-kernel/tests/valuation_credit.rs`
prints 7).

**The sentence the module tells about itself is wrong, and it is the reason
the module is where it is.** Its documentation says: "Neither lib may reach
across to the other … so the composition is the kernel's, which is the only
place allowed to hold both." The first half is right — a lib composing
another lib's domain would be a service in the wrong directory. The second
half is not: every service crate may depend on both libraries, and
`qip-market` already depends on `qip-financial`
(`grep -n 'qip-financial' backend/crates/libs/qip-market/Cargo.toml`). The
kernel is the only place that composes *services* into a cycle; it is not
the only place allowed to compose two libraries. The module doc argued
itself into the runtime from a premise the tree contradicts.

**Where the rest of the plane lives.** The register's §5.3 row scores five of
the plane's six domains reached — term structure, credit, illiquid marks,
cashflow and commitments, corporate actions — and one unreached, the
volatility surface, which ADR 0050 blocks on a licensed option-quote source
and bars from synthetic inputs. Of the five, only credit is composed in
`valuation.rs`. Illiquid marks are `qip_financial::valuation::IlliquidValuator`
called directly from `platform.rs` (`grep -n 'IlliquidValuator' backend/crates/runtime/qip-kernel/src/platform.rs`);
cashflow, commitments and corporate actions are kernel functions over
`Platform` state (`grep -n 'private_holdings_of\|apply_due_corporate_actions' backend/crates/runtime/qip-kernel/src/platform.rs`).
So "the Valuation plane" is one composed type, two direct library calls and
two stateful kernel seams. This record moves the first and names the others.

**What the manifest costs tonight.** `backend/Cargo.toml` lists workspace
members explicitly (`grep -n 'members' -A 3 backend/Cargo.toml`), and a
crate on disk with no member entry is a crate that builds nothing and is
tested by nothing — `architecture.rs::every_service_crate_is_classified_for_money_authority`
asserts equality between the member list and the directory for exactly that
reason. Adding a member is therefore an edit to the one file every lane's
build reads, and it is held by the workspace lane (W7) tonight.

## Decision

### 1. Valuation is a service crate: `backend/crates/services/qip-valuation`, holding `CreditRegister` as it is

The crate takes `valuation.rs` whole — the type, its module documentation
with the false sentence above corrected, the money-and-statistics note that
names the two `f64`/`Decimal` crossing points — and nothing else on the day
it lands. Its dependencies are `serde`, `qip-core`, `qip-financial` and
`qip-market`: the four the module already uses, and no service. `qip-kernel`
gains a dependency on it, constructs it in `Platform::new` and reads it in
`stage_understand` exactly as now, and re-exports it — `pub use
qip_valuation::CreditRegister;` in `lib.rs` in place of `pub use
valuation::CreditRegister;` — so that `qip_kernel::CreditRegister` and
`Platform::credit_register()` keep every existing reader, and
`tests/valuation_credit.rs` stays in `qip-kernel` byte-for-byte, because its
seven tests drive `Platform` and not the type.

**The proof that nothing changed is that the diff has no behavioural line in
it.** A move that needs a test to change is not a move.

### 2. The crate is classified as holding no money authority, and the classification is the first thing the moving lane writes

`qip-valuation` prices; it does not veto, execute, transfer or issue. It goes
in `NO_MONEY_AUTHORITY` in
`backend/crates/tests/qip-acceptance/tests/architecture.rs`, beside
`qip-world-model` and `qip-twin`. The test that holds the list to the
directory fails until it does, which is the intended order: the crate cannot
exist unclassified for one green build.

### 3. The boundary is for the plane, and the plane's other members are named but not moved

The reason for a crate is that the plane's next composition goes in it rather
than in `platform.rs`. Two candidates are named so the moving lane and the
lane after it know what they are looking at, and each is deliberately **not**
in decision 1:

- **Illiquid marks.** `platform.rs` calls `IlliquidValuator::forecast_private_asset`
  and `IlliquidValuator::mark_object` directly and writes the results into
  `Platform::illiquid_marks`. A mark register in `qip-valuation` built from
  the universe, the way `CreditRegister` is, would be the same shape; but
  the call sites read and write `Platform` state today, so moving them is a
  behavioural change with its own proof, not a file move.
- **Cashflow forecasts and corporate actions.** `private_holdings_of` and
  `apply_due_corporate_actions` are stateful kernel seams over the
  commitment book and the position book. They stay.

The volatility surface is not a candidate: ADR 0050 blocks it on a source,
and a crate boundary does not change what it may be wired to.

### 4. The move is a later lane's, and this is the order it does it in

`backend/Cargo.toml` is held tonight. The lane that moves the file, when the
manifest is free, does this and reports each step:

1. Re-read this record and the register's §1.2 and §5.3 rows; if a fact above
   has moved, correct the record before moving anything.
2. Add `"crates/services/qip-valuation"` to `[workspace.members]` and
   `qip-valuation` to `[workspace.dependencies]`; create the crate with the
   four dependencies in §1 and the workspace lints.
3. Move the module into `src/lib.rs` of the new crate with a rename the
   history follows; correct the module doc's sentence; delete
   `mod valuation;` from the kernel and change the re-export.
4. Add `qip-valuation` to `NO_MONEY_AUTHORITY`.
5. Update `CLAUDE.md`: the crate count on the stack line (`ls -d backend/crates/*/*/ | wc -l`
   printed 58 on 2026-09-20 and will print 59) and the `services/` row of the
   layout table, which lists example domains and should name valuation among
   them. ADR 0016's map is by top-level directory and does not change.
6. Run `cargo test -p qip-acceptance --test architecture --test documentation`,
   `cargo test -p qip-kernel --test valuation_credit` (7 passed, and the test
   file unchanged), `cargo clippy --workspace --all-targets` (zero warnings)
   and `./scripts/check-dependencies.sh` (eleven packages, all permitted — a
   workspace crate adds no third-party package), and quote each.
7. Append the move's commit to this record's Consequences.

Until step 3 lands, the register's §1.2 row stays as it is and says this
record exists.

## Consequences

- Register row §1.2 keeps its verdict and now cites this record as the
  decision it asked for; the row's latency clause is corrected at the same
  time (below).
- No file under `backend/`, `frontend/` or `infrastructure/` changes in the
  commit that carries this record. The paper-trading boundary is untouched
  at all three layers — Terraform's ceiling refusal, `AutonomyLevel::deployable`
  at the composition roots, and the type system in `qip-edge`'s `Cell` and
  `qip-cost-router`'s `Determinism::Required` — and the move in §4 touches
  none of them either: a credit register admits no instrument and no order.
- **A correction the row carries from here.** The §1.2 row said "nothing in
  the tree records the latency a plane actually ran at." That is false for
  stages: `grep -n 'STAGE_DURATION_MS' backend/crates/runtime/qip-kernel/src/platform.rs`
  prints two recording sites, and every stage's `elapsed` is observed in
  milliseconds on every cycle under `qip_stage_duration_milliseconds`. What
  is true is narrower and is what the row now says: a stage is not a plane
  — the eight stages cut across the seven planes rather than mapping onto
  them — so the per-plane time scales in §1.2's table are still asserted and
  not measured; and no series of any kind is scraped from any deployed
  process (the observability rule file is the record), so even the stage
  figure reaches nobody.

## What it costs

**A twenty-fifth service crate holding one type.** Five hundred and forty
lines and one `pub struct` behind a crate boundary is a boundary around a
small thing, and it is fair to ask whether it is worth a `Cargo.toml`, a
member entry, a classification line and a documentation count. The answer
this record gives is that the boundary is for what goes in next (§3), and
the reversal condition below is what happens if nothing does.

**The re-export hides the boundary from readers.** Decision 1 keeps
`qip_kernel::CreditRegister` alive so the move is behaviour-free, which
means every `use` line in the tree still names the kernel, and a reader
following imports will not see a service crate. The boundary is real in
Cargo — the kernel cannot be reached *from* the crate — and invisible in
source until a later lane retires the re-export, which it may do only once
`valuation_credit.rs` and `credit_register()` are re-pointed together.

**The plane is still in three places on the day the move lands.** Credit in
the crate; illiquid marks called directly from `platform.rs`; cashflow and
corporate actions as kernel seams. This record makes that explicit rather
than pretending the crate is the plane. A reader who counts "one crate,
therefore one boundary" will be wrong until §3's candidates move, and each
of those is a change somebody proves.

**The manifest is contended.** A workspace-member edit conflicts with every
other lane's edit to `backend/Cargo.toml`. That is why §4 is a sequence for
a later lane rather than a diff in this one, and it is a real delay: the
decision lands tonight and the boundary does not.

**Nothing gets better for the desk.** No expected loss changes, no problem
is reported that was not reported before, no latency moves. The gain is the
one the register named — an edit that reached across the plane would now
be a dependency edge a reviewer sees — and that is the only gain claimed.

## What would make this wrong

- **The crate needs the runtime.** If `qip-valuation` ever needs a type from
  `qip-kernel` — a `Platform` field, a stage outcome, a journal handle — it
  is not a plane's engine but a stage's helper, and the boundaries rule
  forbids the edge in any case. Then fold it back into the kernel and record
  why the plane's composition turned out to need cycle state.
- **Nothing else moves in.** If, two waves after the move, the crate still
  holds `CreditRegister` alone and §3's illiquid-mark composition has not
  been argued in or rejected in writing, the boundary guards one type and
  the crate is ceremony. Then either move the marks in with their proof or
  fold the crate back; leaving it as a one-type crate indefinitely is the
  outcome this record does not want, and it is stated here so that the
  next reader can hold the record to it.
- **The workspace adopts a per-plane crate rule generally.** This record
  decides one crate for one plane from one row's argument. If a later
  decision says every §1.2 plane must be a crate — Cognition is four
  service crates today, Ingestion three — that is a different and larger
  decision, argued from the whole map, and this record is one instance of
  it rather than its precedent.
- **The moving lane finds a behavioural line in its diff.** Then the move is
  not the move this record authorised. Stop, and either find the line's
  cause in a fact above that has changed, or bring the behavioural change
  as its own record.

## Alternatives considered

**Leave it as a kernel module.** Rejected, for the row's reason: the plane
has no boundary, and the module's own documentation shows what that costs
— it justified its location with a claim about the layout that the layout
contradicts, and nothing in the build could tell it so.

**Move it into `qip-market`, which already depends on `qip-financial`.** The
purist's answer, and rejected for two reasons. First, the boundaries rule:
a lib "that composed another lib's domain would be a service in the wrong
directory", and a register that reports problems to a stage is domain-engine
work. Second, and decisive: `grep -l 'qip-market.workspace' backend/crates/*/*/Cargo.toml`
prints eighteen crates including three under `edge/` — `qip-arbitrage`,
`qip-feature-dag`, `qip-routing` — so credit valuation in `qip-market` is
credit valuation in the cell's dependency closure. The cell prices nothing
by covenant and should not carry the code that does.

**Move it into `qip-financial`.** Impossible without a cycle: `qip-market`
depends on `qip-financial`, and `CreditRegister` needs `qip-market`'s
`TermStructure`.

**Move the whole plane at once.** Rejected: illiquid marks, cashflow and
corporate actions read and write `Platform` state, so moving them is a
behavioural change with its own proof, and bundling it with a pure file move
would hide the one inside the other — the exact shape ADR 0016 warned about
in a wide mechanical diff.

**Decide nothing and let the moving lane choose.** Rejected: the row asked
for the decision to precede the move, three lanes have now re-derived the
same facts, and a lane holding the manifest with no decision in hand would
either guess the crate's home or leave it, and both have already happened
once each in this workspace.

## Dependency-direction argument

The new edge is `qip-kernel → qip-valuation`, runtime onto service, which is
the direction the boundaries rule requires. `qip-valuation` depends on three
libraries and no service, so no service gains an edge onto another; no lib
gains an edge onto a service; nothing depends on an app. The acceptance
tests that hold this — `a_library_never_depends_on_a_service_or_an_application`
and `the_dependency_graph_is_acyclic` in `architecture.rs` — run unchanged
against the moved tree, and `every_service_crate_is_classified_for_money_authority`
is the one that fails first if the moving lane forgets §2. The workspace's
third-party set does not move: `scripts/check-dependencies.sh` counts
packages outside the workspace, and a workspace crate is not one.
