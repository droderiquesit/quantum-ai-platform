# Branch disposition, 2026-11-16

Every branch of `ccr-116bbb27-osu8n8` resolved on the owner's instruction of
2026-11-16 ("perform all the required work and drive all branches home"). The
ninety-one generated branches are gone except the nine below; sixty-seven were
verified to have no commits outside `main` before deletion, so nothing was lost
with them. These nine held commits that were never on `main`; each is recorded
with the SHA that restores it, should it ever be wanted:
`git push origin <sha>:refs/heads/<name>`.

## Merged into main after CI passed (11)

Eight report lanes, each answering why its rows cannot be closed from code:

| Lane | PR | Merge |
|---|---|---|
| b53-2 | #278 | `43b5e41` |
| b53-3 | #274 | `f8fcc50` |
| b53-8 | #276 | `ab5705e` |
| b53-13 | #275 | `7934972` |
| b53-14 | #280 | `c0d61b8` |
| b53-15 | #281 | `e571492` |
| b53-17 | #277 | `3801a7f` |
| b53-19 | #279 | `118f9b8` |

Three code lanes, merged only after the whole `ci` run finished green — every
test partition, clippy, format, the gates and the security jobs:

| Lane | PR | Merge | What it adds |
|---|---|---|---|
| b53-5 | #269 | `20d2ff2` | A `MicrostructureDelta` residual cannot lose a field; six families keep their wire names (+159) |
| b53-9 | #273 | `e94f938` | The adversarial perturbation suite is a registry-enforced promotion test (+216) |
| b53-6 | #270 | `4fb4daa` | The widening path runs the back-door estimator; `SIMULATE` journals its draws (+585) |

All three branches were deleted after their merge commit landed.

## Open, not merged: the acceptance partition fails (3)

Each of these passes every other job in `ci` — all test partitions, clippy,
format, the security and supply-chain jobs, and the rule-citation gate — and
fails only `test (acceptance, -p qip-acceptance)`, which exits 100 (nextest:
tests failed). The failing test names are not recoverable from here: job logs
require a signed-in session, the check-run annotations carry only the exit
code, and the runner's log download truncates at the job header. They are left
open rather than merged on a guess.

| Lane | PR | Size | Head when last tested |
|---|---|---|---|
| b53-16 | #268 | +610, book-level basis and gap exposure | `2a48fe40` |
| b53-10 | #271 | +177, lessons only from held episodes | `1f41f3f5` |
| b53-7 | #272 | +1,299, the Symbolic Reasoning Brain | `bd74078d` |

The next step is one command on a machine with the toolchain:
`cargo nextest run -p qip-acceptance` on a checkout of the branch, or opening
the job log in a signed-in browser and reading the `FAIL` lines.

## Refuted by a reader, not merged (4) — branches retained

Merging any of these would put back code two independent readers rejected. Each
finding is already recorded in the decision it belongs to.

| Lane | Refusal | Tip SHA |
|---|---|---|
| b52-26 | Lane 26: `AuthorityGate::permit_new_orders` has no caller outside its tests | `1bb14de937466fc278b68d934d106220f808b162` |
| b53-1 | R12-REASON-1138 refuted | `8d2fe0d5a5f54f25405f93150a4ba69f2fd8aa08` |
| b53-11 | R12-REASON-3003 and R12-QUANT-4042 refuted | `02876ce95a89e6c3acfde6e0c7553f55c48f2a4c` |
| b53-12 | R12-CAPITAL-5067 refuted | `c60b5d956bf6adc0f7377dd728354bb96cc03ccf` |

## Needs a rebase before it can be judged (2) — branches retained

Both conflict with `main` on real code, not only on the index.

| Lane | Content | Conflict | Tip SHA |
|---|---|---|---|
| t50-43 | Divestment under a wallet halt; belief-discrepancy claim (+111) | `docs/adr/README.md` | `ab5f4568a4902c41ccf45ba34ed4bea36092dd5c` |
| t50-45 | Teacher-class blending by regime (MODEL-4006, +712) | `qip-training/src/ensemble.rs`, its tests, the index | `a60890bbddbb4d7f95e94ecce27c8d22befaddec` |

## Deleted as empty number claims (4)

Each held one to three added lines in `docs/adr/README.md` claiming an ADR
number whose body was never written. All ten claimed numbers (0512, 0514, 0516,
0544, 0545, 0546, 0547, 0550, 0551, 0552) are still free on `main`.

b53-44 `45cacc905f11a5235f1c98d7c9472d52587880a5` ·
t51-02 `a6aae6ad5f703dd272e73b46933059f6b36b3644` ·
t51-03 `3ca5a07c3f0daa87caed22cb14f84efee15e4197` ·
t51-05 `29a3a9133632d1f144288cf6db758a48960eeac8`

## Two traps found in the merge gate

1. `scripts/rule-citation.mjs` diffs the branch against the *current* base tip,
   not its merge base. Any branch cut before `main` moved therefore shows
   `main`'s own rule edits in its diff, and is refused for rule changes it never
   made. Bringing the branch up to date with "Update branch" clears it; a
   rebase or a merge of `main` into the branch is the fix, not a prose edit.
   Four of the six code pull requests were also opened with an empty body, which
   the gate refuses independently (`**Rule changed:**` absent).
2. Nothing protects `main`, so a pull request can be merged with a failing `ci`.
   Only the habit of checking each run stands between a red branch and `main`.

## What this does not do

No register row is regraded by any of it. Acceptance and salvage are not
evidence; the traceability matrix scores a row from its assessment file alone
(ARCH-074). Blueprint completion is unchanged at 592 of 1,518.
