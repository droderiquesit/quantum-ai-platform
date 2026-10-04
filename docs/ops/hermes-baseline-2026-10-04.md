# Hermes phase 0 baseline, 2026-10-04

Worktree: `.claude/worktrees/hermes-phase-0`. Nothing committed; no cloud or Terraform state touched.
Worktree carried an uncommitted edit to `backend/crates/apps/qip-cli/src/blueprint.rs` (not mine).

## 1. Rust gates (`backend/`, cargo 1.94.1)

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | exit 0, no output |
| `cargo clippy --workspace --all-targets` | exit 0, 0 `warning` lines, 0 `error` lines; `Finished dev profile ... in 40.12s` |
| `cargo test --workspace --no-fail-fast` | **did not run to completion**: exit 101, 0 `test result:` lines |

Test totals: passed 0, failed 0, ignored 0 (nothing executed). 12 `could not compile` errors, all
`error: linking with cc failed`, each ending `collect2: fatal error: ld terminated with signal 7 [Bus error]`.

Cause: not a missing linker. `cc`, `gcc` and GNU ld 2.42 are installed (`/usr/bin/cc`, `/usr/bin/ld`),
unlike the ADR 0098 premise. The root filesystem is full:
`/dev/mapper/vgzorin-root  232G  218G  2.0G 100% /`. The linker's output is mmap'd, so a full disk
surfaces as SIGBUS. `backend/target` in this worktree is 12G. Not a flake: it is a resource fault,
and a re-run will fail the same way until space is freed. Not freed here (deleting is outside this task).

Failing link targets seen: qip-api (lib test), qip-kernel (models, insights, ledger, asset_classes,
universe_grade, falsification, learning), qip-acceptance (infrastructure, truth_loop, quote_loop, e2e).

## 2. Policy scripts

- `./scripts/check-dependencies.sh`: `dependency policy: 11 third-party package(s), all permitted`
- `./scripts/check-secrets.sh`: `secret scan: nothing found`

## 3. Cloud (read-only)

- `gcloud billing projects describe algorik-dev`: `billingAccountName: ''`, `billingEnabled: false`
  -> **billing is NOT enabled**; no Terraform apply can succeed in `algorik-dev`.
- `gcloud config list`: account `droderiques.it@gmail.com`, project `algorik-dev`, no region/zone set,
  configuration `default`.
- Terraform state: `find . -name '*.tfstate*'` in the worktree found none. Backend/remote state not
  queried (would need cloud access beyond billing describe).

## 4. Tools

| Tool | Result |
|---|---|
| graphify | 0.9.74 (`~/.local/bin/graphify`) |
| ponytail plugin | 4.10.1, user scope, enabled |
| superpowers plugin | 6.4.2 (superpowers-marketplace), user scope, enabled |
| other plugins | pyright-lsp, rust-analyzer-lsp, typescript-lsp 1.0.0 enabled; synced engineering/design/data disabled |
| terraform | v1.15.8 (repo pins 1.9.8 in CI; local differs) |
| gh | logged in as `droderiquesit`, scopes gist, read:org, repo |
| node | v22.23.3 |
| playwright | not installed locally (`npx --no-install playwright --version` -> missing playwright@1.63.0) |

## 5. Workflows (`.github/workflows`)

- `ci.yml` (push, pull_request): the gate. Jobs: format, lint, test, release-build, dependency-policy,
  security-audit, dependency-supply-chain, sbom, frontend-portal, frontend-landing, trivy, trunk,
  infrastructure, secrets.
- `deploy.yml` (workflow_run, workflow_dispatch): gate, images, deploy. Builds/signs/attests and moves
  Cloud Run services; prod refused unless human-dispatched.
- `image.yml` (workflow_dispatch): `bake`, builds the image.
- `infra.yml` (workflow_dispatch): `terraform` plan/up/down; prod refused.
- `vendor.yml` (push, workflow_dispatch): `mirror`, mirrors and scans upstream images.

## Blockers

1. Root disk 100% full (2.0G free): test binaries cannot link. Owner: developer (free space, e.g.
   `cargo clean` in stale worktrees; 12G in this one).
2. `algorik-dev` billing disabled: blocks any apply/deploy. Owner: account owner.
3. Playwright absent; local terraform 1.15.8 vs pinned 1.9.8 (minor).
