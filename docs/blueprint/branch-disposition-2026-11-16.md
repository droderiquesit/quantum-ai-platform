# Branch disposition, 2026-11-16

Every branch of `ccr-116bbb27-osu8n8` resolved on the owner's instruction of
2026-11-16. Ninety-one generated branches are gone except the eight below.
Sixty-seven held no commits outside `main` and were deleted after that was
verified. These eight hold commits that never landed; each is recorded with the
SHA that restores it: `git push origin <sha>:refs/heads/<name>`.

## Merged into main after a full green `ci` run (11)

Report lanes, each answering why its rows cannot be closed from code: b53-2
(#278 `43b5e41`), b53-3 (#274 `f8fcc50`), b53-8 (#276 `ab5705e`), b53-13
(#275 `7934972`), b53-14 (#280 `c0d61b8`), b53-15 (#281 `e571492`), b53-17
(#277 `3801a7f`), b53-19 (#279 `118f9b8`).

Code lanes, merged only once every test partition, clippy, format, the gates
and the security jobs had passed on the merged tree:

| Lane | PR | Merge | What it adds |
|---|---|---|---|
| b53-5 | #269 | `20d2ff2` | A `MicrostructureDelta` residual cannot lose a field (+159) |
| b53-9 | #273 | `e94f938` | The adversarial suite as a registry-enforced promotion test (+216) |
| b53-6 | #270 | `4fb4daa` | Back-door estimator; `SIMULATE` journals its draws (+585) |
| b53-10 | #271 | `60390b9` | A lesson is promoted only from episodes the memory holds (+177, plus the acceptance correction in `90861c1`) |

Each branch was deleted after its merge commit landed.

b53-10 needed one acceptance change before it could land:
`uncertainty_records.rs` promoted a lesson citing `run-1`..`run-6` into an
empty memory, which the lane's own new rule refuses. The test now records the
six episodes first, which is the premise the rule creates, not a weakening of
what it asserts. Verified by reading the refusal path, then by the job.

## Open, not merged: the acceptance partition fails (2)

Both pass every other job in `ci` and fail only
`test (acceptance, -p qip-acceptance)`, which exits 100 (nextest: tests
failed). The failing test names could not be recovered: job logs need a
signed-in session, the check-run annotations carry only the exit code, and the
runner's log download truncates at the job header. Left open rather than merged
on a guess.

| Lane | PR | Size | Head when last tested |
|---|---|---|---|
| b53-16 | #268 | +610, book-level basis and gap exposure | `2a48fe40` |
| b53-7 | #272 | +1,299, the Symbolic Reasoning Brain | `bd74078d` |

Neither branch removes a line of kernel source (b53-16 is purely additive), so
the failure is a premise an existing acceptance test makes, of the kind found in
b53-10 — not a regression in the code path the lane adds. The next step is the
`FAIL` lines from the job log, or `cargo nextest run -p qip-acceptance` on a
checkout of the branch.

## Refuted by a reader, not merged (4) — branches retained

| Lane | Refusal | Tip SHA |
|---|---|---|
| b52-26 | Lane 26: `AuthorityGate::permit_new_orders` has no caller outside its tests | `1bb14de937466fc278b68d934d106220f808b162` |
| b53-1 | R12-REASON-1138 refuted | `8d2fe0d5a5f54f25405f93150a4ba69f2fd8aa08` |
| b53-11 | R12-REASON-3003 and R12-QUANT-4042 refuted | `02876ce95a89e6c3acfde6e0c7553f55c48f2a4c` |
| b53-12 | R12-CAPITAL-5067 refuted | `c60b5d956bf6adc0f7377dd728354bb96cc03ccf` |

## Needs a rebase before it can be judged (2) — branches retained

| Lane | Content | Conflict | Tip SHA |
|---|---|---|---|
| t50-43 | Divestment under a wallet halt (+111) | `docs/adr/README.md` | `ab5f4568a4902c41ccf45ba34ed4bea36092dd5c` |
| t50-45 | Teacher-class blending by regime (MODEL-4006, +712) | `qip-training/src/ensemble.rs`, its tests, the index | `a60890bbddbb4d7f95e94ecce27c8d22befaddec` |

## Deleted as empty number claims (4)

Each held one to three lines in `docs/adr/README.md` reserving an ADR number
whose body was never written. All ten claimed numbers (0512, 0514, 0516, 0544,
0545, 0546, 0547, 0550, 0551, 0552) are still free.

b53-44 `45cacc905f11a5235f1c98d7c9472d52587880a5` ·
t51-02 `a6aae6ad5f703dd272e73b46933059f6b36b3644` ·
t51-03 `3ca5a07c3f0daa87caed22cb14f84efee15e4197` ·
t51-05 `29a3a9133632d1f144288cf6db758a48960eeac8`

## Two traps found in the merge gate

1. `scripts/rule-citation.mjs` diffs the branch against the *current* base tip,
   not its merge base, so a branch cut before `main` moved is refused for rule
   changes it never made. Bringing the branch up to date clears it. Four of the
   six code pull requests were also opened with an empty body, which the gate
   refuses independently.
2. Nothing protects `main`, so a pull request can be merged with a failing `ci`.

No register row is regraded by any of this. Blueprint completion is unchanged at
592 of 1,518 (ARCH-074).
