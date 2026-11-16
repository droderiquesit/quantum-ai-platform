# Branch disposition, 2026-11-16

Every branch of `ccr-116bbb27-osu8n8` resolved on the owner's instruction of
2026-11-16 ("perform all the required work and drive all branches home"). The
ninety-one generated branches are gone except the twelve below; sixty-seven were
verified to have no commits outside `main` before deletion, so nothing was lost
with them. These twelve held commits that were never on `main`, and each is
recorded here with its tip SHA, so any of them can be restored with
`git push origin <sha>:refs/heads/<name>`.

## Merged into main (8) — branches deleted

| Lane | PR | What it records |
|---|---|---|
| b53-2 | #278 | Four R12-EXPAND rows examined, none closable in code |
| b53-3 | #274 | Four rows not closed, none built |
| b53-8 | #276 | Six rows judged, none built |
| b53-13 | #275 | One row verified, five stopped with the reason |
| b53-14 | #280 | Five rows triaged, none closable by code |
| b53-15 | #281 | None of its five rows can reach COMPLETE from code |
| b53-17 | #277 | Why four rows are not closable from this repository |
| b53-19 | #279 | R12-CONTRACT-7069 needs an owner decision, not code |

Tip SHAs: `43ec383de6677dbb7cf3a8ef962601c3904b86f1` (b53-2),
`d256ea8b5ad5bf594990138bb4133d97473c9a35` (b53-3),
`687f4333f2b6e0590115b8be76e2433b9cf87729` (b53-8),
`fbd0928f3385a07cac3beac5fd723052c32b3369` (b53-13),
`e4621355a27fc2d7a76dbb362173a7a2207e04c2` (b53-14),
`32d6058430de87b808f9fba48da15caad148a771` (b53-15),
`d7fba2abc27c5b398565fe333488aec95b3a01ab` (b53-17),
`f235c80959b5e904e8bb2d9e2d6d00b8edea0bda` (b53-19).

## Open pull requests, awaiting CI (6) — branches retained until merged

These were cut when wave b53's round ended; no reader refuted them. Each was
checked against `main` with `git merge-tree` (zero conflicts), for tests added,
for skipped tests, for removed assertions and for lint suppression. CI is the
judge of whether they compile and pass; none has been merged on a local build.

| Lane | PR | Change | Tip SHA |
|---|---|---|---|
| b53-16 | #268 | Basis/gap exposure reported at book level (+610) | `a1c202f28b404b051f63aa98c114df3aadef20ad` |
| b53-5 | #269 | MicrostructureDelta residual and wire names (+159) | `76c4d746f8aa149833bdcc10d99484e228e0ce8f` |
| b53-6 | #270 | Back-door estimator, SIMULATE draw journal (+585) | `a1a6dbfc2c5d95066071442e4368b74533f090d6` |
| b53-10 | #271 | Lessons promoted only from held episodes (+177) | `60d0dc5af3602e1685e07b54dc892691407216f0` |
| b53-7 | #272 | Symbolic Reasoning Brain in the organisation (+1,299) | `b89fd79c1be8f6c9598c05e0922987294b0adabd` |
| b53-9 | #273 | Adversarial suite as a promotion test (+216) | `a5577720912bfb10c188520b1a8951e959282286` |

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
0544, 0545, 0546, 0547, 0550, 0551, 0552) are still free on `main`, so nothing
is lost and any future lane may claim them again.

b53-44 `45cacc905f11a5235f1c98d7c9472d52587880a5` ·
t51-02 `a6aae6ad5f703dd272e73b46933059f6b36b3644` ·
t51-03 `3ca5a07c3f0daa87caed22cb14f84efee15e4197` ·
t51-05 `29a3a9133632d1f144288cf6db758a48960eeac8`

## What this does not do

No register row is regraded by any of it. Acceptance and salvage are not
evidence; the traceability matrix scores a row from its assessment file alone
(ARCH-074). Blueprint completion is unchanged at 592 of 1,518.
