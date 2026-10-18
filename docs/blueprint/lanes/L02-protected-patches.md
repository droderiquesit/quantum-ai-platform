# L02 patches that need a protected path (2026-10-18)

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
