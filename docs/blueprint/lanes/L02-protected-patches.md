# L02 patches that need a protected path (2026-10-18, patch 2026-10-19)

**Status 2026-10-19.** Five of the seven rows are now a ready patch,
`docs/blueprint/lanes/L02-owner-patch-2026-10-19.patch`, built in a scratch
worktree on `ccr-d1bc1e54-kf6mr0` @ `ebf08fa` and checked to apply at
`f8df035`. Two rows are left out (below). Nothing protected is modified on the
branch: the owner applies the patch. Not validated by a workflow run:
`.github/**` cannot be exercised from a lane.

## Apply (owner)

```
git fetch origin ccr-d1bc1e54-kf6mr0 && git checkout ccr-d1bc1e54-kf6mr0 && git pull --no-rebase
git apply --check docs/blueprint/lanes/L02-owner-patch-2026-10-19.patch
git apply docs/blueprint/lanes/L02-owner-patch-2026-10-19.patch
git commit -am "Apply L02 owner patch (R12-ARCH-9071, CICD-7116/7120/9085, FINOPS-10134)"
```

The patch holds the protected edits and their paired test edits together:
neither may land without the other.

## Per row

| Row | Result |
|---|---|
| R12-ARCH-9071 | In patch. Four guard suites get their own CODEOWNERS lines; `architecture.rs::the_guard_suites_are_owned_by_name_in_codeowners`. |
| R12-CICD-7116, R12-CICD-7120 | In patch. `deploy.yml` gains `simulation-gate` and `paper-gate` (each `needs: gate`, checkout of `DEPLOY_SHA`); `images` now `needs: [gate, simulation-gate, paper-gate]`. `prod_guard.rs` asserts the list and each gate's own `needs: gate`. Limit kept: the gates run source tests, not the built image; Kargo's `paper` stage stays the manual gate. |
| R12-CICD-9085 | In patch. `ci.yml` `test` job, `acceptance` partition only: runs `tick_replay` and `performance` into `replay-bench.txt` (`shell: bash` so `tee` cannot hide a failure) and uploads artifact `replay-bench`, 90 days, `if: !cancelled()`. `pipeline_scripts.rs` asserts it. Gap kept: a log, not a delta; no committed baseline exists. |
| R12-FINOPS-10134 | In patch, reduced. `variable "owner"` (validated lowercase label, **no default** so a forgotten tfvars line fails the plan rather than guessing), `owner = "platform"` in all four tfvars, `owner` and `region` added to `local.labels`, `owner = "platform"` added to the 13 `*.tftest.hcl` top-level `variables` blocks (a required variable would otherwise fail them). `terraform_contract.rs` asserts it. **Left out:** the per-module-call `merge(local.labels, {plane, workload})`; 17 call sites whose plane and workload values nobody has chosen. Applying changes labels in place on existing resources (an update, not a replace). |
| R12-CICD-9108 | **Left out.** The spec itself says `gcloud artifacts sbom load` acceptance of this repository's CycloneDX output is unconfirmed, and it needs a new `actions: read` grant on `images`. An unverified step that can fail every deployment is a risk to add blind. Open for the owner: confirm the format, then re-spec. |
| R12-GCP-9120 | **Left out.** The write of `infrastructure/change-windows.json` and the gate script was refused by the auto-mode classifier ("Instruction Poisoning"); per the lane rules the row stopped and was not retried another way. The choice made, for the owner to apply or edit: network, project and database changes only Mon-Thu 14:00-20:00 UTC, no window Fri-Sun, `up` action only (teardown deliberately ungated). Nothing for this row is in the patch. |

## Choices made

- Change windows (not in patch, see above): Mon-Thu 14:00-20:00 UTC; none Fri-Sun.
- `owner` label value `var.owner`, validated `^[a-z][a-z0-9_-]{0,62}$`; each environment states `owner = "platform"` as a placeholder the owner fills in. No personal identifier.
- Gates only add: no check, job, need, refusal or prod guard was removed or relaxed. `prod_guard.rs` replaced `deploy.contains("    needs: gate\n")` with a stricter assertion on the `images` list plus each new gate's own `needs: gate`.

## Tests run in the scratch worktree (all `cargo test -p qip-acceptance --no-fail-fast --test ...`)

```
architecture       test result: ok. 56 passed; 0 failed
cicd_governance    test result: ok. 5 passed; 0 failed
gitops             test result: ok. 47 passed; 0 failed
infrastructure     test result: ok. 153 passed; 0 failed
pipeline_scripts   test result: ok. 9 passed; 0 failed
prod_guard         test result: ok. 3 passed; 0 failed
terraform_contract test result: ok. 12 passed; 0 failed
terraform fmt -check -recursive terraform environments   exit 0
terraform init -backend=false; terraform validate        Success! The configuration is valid.
terraform test (infrastructure/terraform)                Success! 103 passed, 0 failed.
```

Terraform 1.9.8 (the repository pin), SHA-256 checked against HashiCorp's sums.
No script changed, so `node --test` did not apply.

Mutations, each failed for the right reason and was restored byte-identical:
delete the `lane_placement.rs` CODEOWNERS line (architecture.rs:4141); drop
`paper-gate` from `images`' needs (prod_guard.rs:285); change `paper-gate`'s
`needs: gate` (prod_guard.rs:307); rename the `replay-bench` artifact
(pipeline_scripts.rs:586); delete `owner = var.owner` from `local.labels`
(terraform_contract.rs:1137); delete `owner` from the stage tfvars
(terraform_contract.rs:1154).

## Files the agent-boundary check will list

Protected: `.github/CODEOWNERS`, `.github/workflows/ci.yml`,
`.github/workflows/deploy.yml`, `infrastructure/environments/{dev,test,stage,prod}/terraform.tfvars`,
`infrastructure/terraform/variables.tf`.
Not protected, same patch: `infrastructure/terraform/main.tf`, 13
`infrastructure/terraform/tests/*.tftest.hcl`, and
`architecture.rs`, `prod_guard.rs`, `pipeline_scripts.rs`, `terraform_contract.rs`
under `backend/crates/tests/qip-acceptance/tests/`.

# Original specs (2026-10-18), kept for the two rows left out and for reference

None of these is applied. Each row below stays PARTIAL until the owner applies
its patch together with the paired test edit, because a test asserting the
change fails on the branch while the protected file is unchanged. Not
validated by a workflow run: `.github/**` cannot be exercised from a lane.

## R12-ARCH-9071 (guard suites unprotected)

`.github/CODEOWNERS`, after the `backend/crates/edge/qip-edge/src/cell.rs` line:

```
# The guard suites themselves: the wildcard alone lets one reviewer weaken them
backend/crates/tests/qip-acceptance/tests/architecture.rs @droderiquesit
backend/crates/tests/qip-acceptance/tests/paper_boundary.rs @droderiquesit
backend/crates/tests/qip-acceptance/tests/lane_placement.rs @droderiquesit
backend/crates/tests/qip-acceptance/tests/arch_001_independent_reflex.rs @droderiquesit
```

Paired test (architecture.rs): parse CODEOWNERS, strip comments, and fail for
each of the four paths unless a non-`*` pattern equal to the path exists.
Mutation: delete one line.

## R12-CICD-7116 and R12-CICD-7120 (named simulation and paper gates)

`.github/workflows/deploy.yml`, two jobs after `gate:` (same `runs-on`,
checkout and toolchain steps as the workspace test job in ci.yml):

```
  simulation-gate:
    needs: gate
    # run: cargo test -p qip-acceptance --no-fail-fast --test arch_056_simulation_engine --test arch_042_twin_simulation
  paper-gate:
    needs: gate
    # run: cargo test -p qip-acceptance --no-fail-fast --test paper_boundary --test arch_007_paper_boundary --test prod_guard
```

and in `images:` replace `needs: gate` with `needs: [gate, simulation-gate, paper-gate]`.

Paired test edit, `prod_guard.rs` (~:275-282): the assertion
`deploy.contains("    needs: gate\n")` must become an assertion that the
`images` job's `needs:` list names all three jobs, and the `gate_job` split on
`"\n  images:\n"` must split on `"\n  simulation-gate:\n"`. Mutation: drop
`paper-gate` from the list. Both suites named exist today
(`ls backend/crates/tests/qip-acceptance/tests`). Honest limit for 7120: the
gate runs the paper tests, not the built image, and Kargo's `paper` stage stays
the manual gate the register already records.

## R12-CICD-9085 (replay and benchmark deltas)

`.github/workflows/ci.yml`, in the job that runs `cargo test --workspace`, a
step after it: run `cargo test -p qip-acceptance --test tick_replay --test performance -- --nocapture 2>&1 | tee replay-bench.txt`
then `actions/upload-artifact@v4` with `name: replay-bench`, `path: replay-bench.txt`,
`retention-days: 90`, beside the `sbom` artifact (ci.yml:360). Paired test in
`pipeline_scripts.rs`: ci.yml contains the upload step naming `replay-bench`.
Gap kept: the output is a log, not a delta against a baseline; a delta needs a
committed baseline file that does not exist.

## R12-CICD-9108 (SBOM attached to the image)

`.github/workflows/deploy.yml`, after the sign-and-create step: download the CI
`sbom` artifact for the gated SHA and run
`gcloud artifacts sbom load --source <cdx.json> --uri <image>@<digest>`.
Paired assertion in `gitops.rs` beside
`the_pipeline_no_longer_moves_a_service_and_still_signs_attests_and_refuses_prod`.
Open question for the owner: `sbom load` takes SPDX or CycloneDX only for
images the registry has scanned; confirm the repository's format is accepted
before relying on it.

## R12-GCP-9120 (change windows)

Needs three files: `infrastructure/change-windows.json` (windows per resource
class), a gate step in `.github/workflows/infra.yml` reading the saved plan's
resource addresses (`terraform show -json plan`) and refusing network, project
or database changes outside a window, and an assertion in
`cicd_governance.rs` beside
`terraform_apply_and_destroy_consume_saved_plans_not_auto_approve`. The window
data is a policy decision the owner has not made; no lane should invent one.

## R12-FINOPS-10134 (labels)

Needs `infrastructure/terraform/variables.tf` (a validated `owner` variable)
and each environment tfvars (`infrastructure/environments/**`), both protected.
Unprotected half, in `main.tf`: add `region = var.region` to `local.labels`
and `merge(local.labels, { plane = ..., workload = ... })` at each module call.
Without the owner variable the scan test cannot demand five keys.
