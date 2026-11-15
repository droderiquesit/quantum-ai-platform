# Wave b52: the larger builds, one lane per group of rows

Written 2026-11-14 by the wave580 orchestrator for the build sessions it starts.
You are one lane. Your lane number is in your first message. Your rows are the
entry under that number in `docs/blueprint/lanes/b52-rows.json`.

## What a lane does

For each of your rows, in order:

1. Read the requirement: the object with that `id` in
   `docs/blueprint/requirements/R12-*.json` (`statement`, `verification.check`).
   Read the register row with the same id in `docs/blueprint/assessment/R12-*.json`
   (`current`, `gap`, `evidence`). The rows file adds what the last reader said
   is missing.
2. List every clause of the statement and of the check. Each one needs
   production code a composition root or the cycle reaches on a committed path,
   and a test whose assertion can fail.
3. Build what is missing. Prefer a new module over growing
   `qip-kernel/src/platform.rs`; thirty-seven other lanes are editing this tree
   at the same time and that file is where they collide.
4. Mutation-verify every new test: break the production line it guards, run it,
   read the failure, restore from a saved copy, confirm with `cmp`, run green.
5. Run `cargo fmt --all --check`, `cargo clippy -p <each crate you touched>
   --all-targets -- -D warnings`, the tests of each crate you touched with
   `--no-fail-fast`, and `./scripts/check-secrets.sh`. Read the output.
6. Commit that row alone. The message says why, names the failure the code
   prevents, and quotes the evidence (the `test result:` line, the mutation and
   its failing line).
7. Push to your own branch after every row, so finished work is never lost if
   the session stops.

A row too large to finish is not skipped: build the largest slice that is
honest by itself, commit it, and say exactly what remains.

## What a lane does not do

- **No live-order path, ever.** The platform is paper trading only. Do not
  create, enable or ease one, and do not touch the three layers that hold it:
  the Terraform refusal in `infrastructure/terraform/variables.tf`,
  `AutonomyLevel::deployable` at the composition roots, and the `Cell`
  constructor. If a row seems to need a live path, stop that row and report it.
- **No protected path.** Do not edit `.github/**`, `.claude/**`, any
  `CLAUDE.md`, `scripts/check-*.sh`, `scripts/dependency-allowlist.txt`,
  `scripts/agent-*`, `scripts/sast-*`, `scripts/lanes/**`,
  `infrastructure/terraform/variables.tf`, `execution-nodes-enabled.tf`,
  `build-pool.tf`, `infrastructure/environments/**`, any existing file under
  `docs/adr/`, or the four guard suites (`architecture.rs`, `paper_boundary.rs`,
  `lane_placement.rs`, `arch_001_independent_reflex.rs`). A row that needs one
  gets a patch file under `docs/blueprint/lanes/` and the outcome `NEEDS_PERSON`.
- **No register edits.** Do not touch `docs/blueprint/assessment/**` or
  `traceability-matrix.md`. Thirty-eight lanes editing the same JSON would not
  merge. You write one report file instead (below) and the orchestrator grades.
- **No new dependency.** Nothing outside `scripts/dependency-allowlist.txt`.
- **No test weakened.** Never delete, `#[ignore]`, or loosen an existing test or
  guard to get green. A test that fails after your change is evidence about
  your change.
- **No invented figure.** A rate, limit or term nobody gave is not chosen by a
  lane. Report `NEEDS_FIGURE` and say which.
- **No cloud action.** No Terraform apply, no workflow dispatch, no deploy.

## Architectural decisions

If a row needs a decision an existing ADR does not record, write a new ADR. Your
lane owns two numbers and no others: `0600 + 2 x lane` and the next one (lane 07
owns 0614 and 0615). Do not add a row to `docs/adr/README.md`; the orchestrator
adds the index rows. Follow the form of the existing records.

## Risk, execution, capital, portfolio, broker, routing and compliance crates

Read `.claude/rules/01-security-and-safety.md` and
`.claude/rules/domains/risk-and-execution.md` first. A change under those
crates' `src/` also runs `cargo test -p qip-acceptance --test compliance_proof
--test security --test paper_boundary`, proves any new limit can fire, and
states in the commit that the paper boundary is intact, naming the three layers.

## The report

When every row has been attempted, write
`docs/blueprint/lanes/b52-<lane>.json`, commit and push it:

```json
{"lane": "07", "rows": [{
  "id": "R12-...",
  "outcome": "BUILT | SLICE | NOT_BUILT | NEEDS_PERSON | NEEDS_FIGURE",
  "claims_requirement_met": true,
  "what": "two sentences",
  "remains": "what is still open, or empty",
  "tests": ["path::test_fn"],
  "test_command": "cargo test -p ... --test ...",
  "mutations": ["file, what was changed, the quoted failing line"],
  "commit": "short hash"
}]}
```

`claims_requirement_met` is true only when every clause has its code and its
test. It is a claim, not a grade: two independent readers and the pipeline
decide, and a row is not COMPLETE until they have.

## Evidence

Never say a check passed unless you ran it and read its output. A check that did
not run is reported as not run.
