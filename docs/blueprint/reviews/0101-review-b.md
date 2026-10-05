# ADR 0101 review B: feasibility and cost

Reviewer lens: can what 0101 adopts be built under two dependencies, no async
runtime, and a 25 USD/day ceiling. Body read from commit `24af6bae`
(`docs/adr/0101-blueprint-v12-0-...md`); greps run in the worktree on
`docs/hermes-phase-0`.

## Verdict: APPROVE-WITH-CHANGES

The decision is direction-only and authorises no execution, so it creates no
build obligation and cannot itself breach any of the three constraints. The
changes are because three repo-state claims fail their greps and because the
cost constraint the reviewer was asked to test does not exist in the record.

## Repo-state claims checked

| Claim in ADR | Command | Result |
|---|---|---|
| `blueprint.rs` `cite()` "already maps" `M12`/`G3` (Decision 2; Validation) | `grep -n '"M12"\|"G3"' backend/crates/apps/qip-cli/src/blueprint.rs` | **No output.** `cite()` (line 356) maps only `Some("M")`->"v11.6", `Some("G")`->"GCP v2.1", `Some("H")`->"diagram"; anything else falls to `text(&source["doc"])`. The claim is false in this tree, and the Validation bullet would fail as written. |
| `the_adr_index_links_a_body_for_every_claimed_number` exists in `documentation.rs` | `grep -rn "the_adr_index_links" backend/crates` | **No output.** Only `every_internal_link_resolves` exists (`documentation.rs:804`). |
| Index holds a 0101 row | `grep -c "0101" docs/adr/README.md` | `0` in this tree. Body exists only on commit `24af6bae`. |
| `execution_nodes = {}` in every environment | `grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars` | Holds: stage:49, dev:227, test:49, prod:51 all `= {}` (dev:200 is a commented example). |
| Execution-node module accepts only C3/C3D | `grep -n -i "c3d\|c4d" .../execution-node/main.tf` | Holds (header line 4: "one dedicated C3 or C3D"). |
| Dependency state | `./scripts/check-dependencies.sh` | `dependency policy: 11 third-party package(s), all permitted`. ADR adds none. |
| No async runtime | `grep -rlE "tokio\|async fn" backend/crates --include=*.rs --include=Cargo.toml` | No output. Holds. |
| `AutonomyLevel::deployable` | `grep -rn "fn deployable" backend/crates` | `qip-risk-engine/src/autonomy.rs:110`. Holds. |
| `qip-contracts::capital`, `event_fabric_schema_lock` (C14) | `ls .../qip-contracts/src`; `grep -rl event_fabric_schema_lock` | Both exist. Holds. |
| All 17 cited ADR numbers have a body | `ls docs/adr` loop | All 17 present. |
| Cost ceiling | `grep -rniE "25 ?USD\|\$25\|usd.?per.?day\|cost ceiling\|daily.?(cap\|ceiling)" docs/adr .claude CLAUDE.md docs/ops` | **No 25 USD/day figure anywhere.** Only 0098:165 "The owner has set a monthly cost ceiling" (as a reopening condition, unstated) and `.claude/model-integration.md:67` "Daily Budget: $1.00" (model-call budget, unrelated). |

## Feasibility against the three constraints

- **Two dependencies / no async.** Fine for direction. ADR correctly leaves C2
  (Tokio/QUIC/gRPC/accelerator clients) closed. Note the ADR's own count
  language: the tree has 11 third-party packages in the lock; the permitted
  set is what the script says, not "two crates" literally. Not an ADR 0101
  defect, but a future reader will conflate them.
- **What can be built in-process under those rules.** Only C9 shadow logic,
  C10's router over existing substrates, the in-tree NOW state estimator and
  lattice (blocking, deterministic), and C12's restrict-only limit feeding the
  Gate. All are plain Rust with `BTreeMap`; no blocker.
- **What cannot, regardless of money.** Anything needing GPU/TPU, Vertex Agent
  Engine, Spanner/Bigtable/BigQuery, Cloud Batch or an ML framework is blocked
  by C2/C4/C5 (dependency class) before cost is reached. The ADR says so.
- **25 USD/day.** Not testable: no record sets it, and the ADR (C11) says v3.0
  states no cost figure and the owner has set none. Arithmetic that matters if
  the ceiling is real: a reserved TPU slice or GPU pool exceeds 25 USD/day by
  itself on any list price I am aware of, and "thousands of concurrent models"
  and "hundreds/thousands of ephemeral agent workers" cannot fit; the
  claim in C11 that only "bounded in-process logic in an existing binary"
  fits is consistent with that. Also the dev project has billing disabled
  (C8), so the true available spend today is zero.
- **Scale mismatch.** "Thousands of models" cannot run on the single-desktop
  loop (ADR 0098) under any ceiling. The ADR admits this in "What it costs".

## Required changes

1. Remove or correct the claim that `blueprint.rs` `cite()` already maps
   `M12`/`G3` (Decision 2 and the Validation bullet). In this tree it maps
   only `M`, `G`, `H`. Either land that change first (not by this lane;
   `blueprint.rs` is out of bounds here) and cite its commit, or state it as a
   required follow-up and drop the grep from Validation until it passes.
2. Fix the Validation section's test name: `the_adr_index_links_a_body_for_every_claimed_number`
   does not exist. Name the test that does, or say it is to be written.
3. Add the 0101 row to `docs/adr/README.md` in the same change as the body,
   or the index and body disagree on the branch.
4. Record the cost constraint explicitly. Either state the owner's daily
   ceiling (if 25 USD/day is it, cite where it was given, since no file holds
   it) in C11, or state that none is recorded. As written, C11's "owner cost
   ceiling" is unfalsifiable and ADR 0098 speaks of a monthly one; reconcile
   monthly versus daily.
5. In C11, add the one-line consequence that until a ceiling exists the
   budget for the Superintelligence Plane is zero cloud spend (dev billing
   disabled), so every v12 item is in-process shadow work only. This turns
   "waits on the owner" into a testable rule.
6. Qualify "two dependencies" wherever the ADR leans on it: the enforced
   fact is `check-dependencies.sh` printing "all permitted" (currently 11
   third-party packages), not a count of two.

## Not done

No build, no tests run (the ADR changes no code). I did not read the v12
delta reports or blueprint sources, so the "23 subsections" and "eleven
unknowns" figures are unchecked by me. Nothing committed beyond this file.
