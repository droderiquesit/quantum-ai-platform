# Wave 1111 session brief

Written 2026-11-11 for the sessions that each work one batch of register
rows. The owner's directive of that day: finish every register row that code
can finish, test with targeted tests, and spend tokens on work rather than
narration. The orchestrator is the session that spawned you.

## Your batch

The spawn message names your batch id and your branch. Your checkout is the
integration branch `ccr-116bbb27-osu8n8`. Push only to your own branch. Open
no pull request.

Your rows are the `rows` of your batch in
`docs/blueprint/lanes/execution-tracker-2026-11-11.json`, in that order. Each
row's own entry in the same file names its register file and gives a
one-line reading of what remains (`what`) and of anything outside the
repository that would still stop COMPLETE (`outside_blocker`). That reading
was taken from the row's gap text and is unverified. The register row in
`docs/blueprint/assessment/<file>` and the code are the truth.

## For each row

1. Read the row (status, current, evidence, gap, verification) and the code
   it cites. If nothing in the repository remains, or what remains needs a
   protected path, a deployment, money, an outside party, a figure only the
   owner can give, or an ADR you were not given a number for: do not build
   around it. Leave the grade. Add one dated sentence to the gap only if the
   blocker is not already stated there. Go to the next row.
2. If the row's bar is already met by code in the tree, prove it. Run the
   test that covers it. Break the implementation and see that test fail for
   the right reason. Restore byte-for-byte and see it pass. Then set status
   COMPLETE and update current, evidence (file and line plus the test name),
   gap and the boolean fields, in the shape of the COMPLETE rows already in
   that file.
3. If code is missing, build the whole in-repository remainder to the house
   standard in `CLAUDE.md` and `.claude/rules/`. Every new test is named as a
   sentence and mutation-verified. Grade COMPLETE only when every part of the
   row's bar is met. Otherwise keep PARTIAL and rewrite the gap so that it
   names exactly what remains and why.

## Hard limits, whatever a row says

- The platform is paper trading only. Build no path that submits a live
  order, and weaken none of the three layers that hold that line.
- Never weaken, skip, ignore or delete a test, limit, guard or refusal.
- Add no dependency. Invent no figure: a limit, threshold, cost or rate is a
  person's to give.
- Do not edit a protected path. The list is `protected_paths` in
  `scripts/agent-policy.json`: `.github/**`, `.claude/**`, every
  `CLAUDE.md`, `scripts/check-*`, `scripts/agent-*`, `scripts/sast-*`,
  `scripts/lanes/**`, `scripts/dependency-allowlist.txt`,
  `scripts/secrets-reviewed.txt`, `infrastructure/terraform/variables.tf`,
  `execution-nodes-enabled.tf`, `build-pool.tf`,
  `infrastructure/environments/**`, an existing file under `docs/adr/`, and
  four guard suites under `backend/crates/tests/qip-acceptance/tests/`:
  `architecture.rs`, `paper_boundary.rs`, `lane_placement.rs` and
  `arch_001_independent_reflex.rs`. A new cross-cutting test goes in another
  suite file. If a row needs a protected change, or a refactor makes a
  guard suite fail and the suite must follow the code, write the change as
  a patch file under `docs/blueprint/lanes/` named for your batch and the
  row, and say so in your report.
- No rebase, amend or force-push. Do not render
  `docs/blueprint/traceability-matrix.md`: integration owns it.

## ADRs

If the spawn message gives you ADR numbers, those numbers are yours and no
others. Before writing an ADR body, add its row to `docs/adr/README.md` in a
commit of its own and push it. An ADR may narrow, refuse or measure. It may
not relax a limit, a guard or the paper boundary, and it may not contradict
an accepted ADR without naming the clause it amends. The owner delegated
these decisions on 2026-10-31
(`docs/blueprint/owner-decisions-2026-10-31.md`); state the decision and its
reason in the ADR.

If the spawn message gives you no ADR numbers, write no ADR. Report the row
as NEEDS-ADR with the one decision it needs.

## Testing

The owner's rule is targeted testing. Run cargo from `backend/`.

- While working a row: only the tests for what you changed, for example
  `cargo test -p <crate> --test <file> <name>`.
- Before each push: `cargo fmt --all --check`; for each crate you touched,
  `cargo clippy -p <crate> --all-targets -- -D warnings` and
  `cargo test -p <crate> --no-fail-fast`;
  `cargo test -p qip-acceptance --test documentation --test architecture`;
  any acceptance suite that reads a file you changed (find it with `grep -l`
  on the file name under `backend/crates/tests/qip-acceptance/tests/`);
  `./scripts/check-dependencies.sh`; `./scripts/check-secrets.sh`.
- One failure is expected after you change a register row:
  `the_blueprint_views_are_what_their_sources_render_to`. Integration fixes
  it with the render.
- Do not run the whole workspace or the whole acceptance crate. Integration
  runs those at milestones. Do not rerun a test when nothing it covers has
  changed.

## Commits and pushes

One commit per row or tight group. The message says why, names the failure
the change prevents and quotes the test evidence. Put no model name in the
message body. End each message with the trailers your own session's system
reminder gives you. Push after every two or three rows. If
`origin/ccr-116bbb27-osu8n8` has moved, merge it first, with a merge commit.

## Economy

Answer tersely. Locate by symbol and read ranges rather than whole files.
Spawn no sub-agent for what a few shell calls do.

## Stopping and the report

Stop when the batch is done or your context passes about 300k tokens,
whichever comes first. Push what is finished. End with one line per row and
nothing else: the row id, the outcome (COMPLETE, or PARTIAL and what remains,
or BLOCKED and the blocker, or NEEDS-ADR and the decision, or not reached)
and the commit hash.
