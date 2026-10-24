# The COMPLETE bar, as one rater applies it (AUDIT-5, 2026-10-24)

AUDIT-2, AUDIT-3 and AUDIT-4's three lanes graded the same population against
the same sentence in `lanes/README.md`. On disjoint random draws they found
overclaim rates of 44%, 18% and 10%, a spread wider than sampling explains. The
reports show why. The lanes read three parts of the bar differently: which test
must fail, what counts as a default path, and how to grade a prohibition whose
guard is built only alongside a gated feature. This file fixes one reading of
each part, and `audit-5-2026-10-24.json` lists the rows to re-grade against it.
A row is COMPLETE only when all four parts hold.

## 1. A test fails when the core line is broken

**Some test in the repository must fail when the core line is mutated.** It does
not have to be the test the row's `verification` names.

- If the named test survives and another test fails, the behaviour is proven
  and only the row's record is wrong. Correct `verification` and `evidence` to
  name the test that failed, keep the status, and say so in `notes`. A
  citation error is not an implementation gap.
- If no test fails, the row is PARTIAL. "No test fails" means the mutation was
  run against every test binary of the crate that owns the core line, plus the
  binaries the row names. "Some suite I tried passed" does not count.
- A test that hangs under the mutation counts as failing only if it is bounded
  by a timeout that turns the hang into a failure. An unbounded hang is a
  finding, not a pass and not a proof.
- When the requirement names its own proof, for example "a mutation of stage X
  changes the replay digest", that proof must hold. Another test failing is not
  a substitute.

## 2. The behaviour runs on a default path

- **Counts:** the default code path of a shipped binary (`backend/crates/apps/*`),
  or a committed deployment. A shipped binary's default path counts even when no
  environment deploys that binary today. The edge node and an operator-run
  `qip-cli` command are examples.
- **Does not count:** a path behind an env var or tfvar that every committed
  environment leaves off or null.
- **Emission and collection are separate clauses.** If the requirement asks that
  a series is emitted or recorded, emission on a default path meets it. If it
  asks that the series is monitored, scraped, alerted on or charted, nothing
  short of a collector meets it. `docs/ops/observability` and
  `modules/observability/NOT-SCRAPED.md` record that no collector exists.

## 3. Prohibitions and gated guards

The question is whether the forbidden outcome can happen on a default path
without passing the guard. Whether the guard runs by default is not the question.

- **COMPLETE:** the guard is built in the same place as the only path that could
  produce the outcome, so the outcome cannot occur without the guard. The guard
  is proven by part 1. Example: a refusal constructed inside the plan loader,
  where the plan loader is the only way the forbidden instruction can enter.
- **PARTIAL:** any default path reaches the outcome without the guard. Also
  PARTIAL: the guard is optional on a path that is otherwise live.
- **Positive capabilities stay under part 2.** A gated feature that is off is
  not COMPLETE because it is harmless.

## 4. The requirement's text, not the row's restatement

A clause that the row, an ADR or a code comment withdrew is unmet, unless the
requirement's own text allows withdrawing it. When an ADR declined a clause, the
row stays PARTIAL and its `gap` cites the ADR: the owner decides whether the
requirement changes.
