# Security baseline — 2026-09-25

Taken at `899e6ece` on `claude/blueprint-v11-6-program`, which is `main` at
`53fc1f42` plus documentation-only commits. This is the M0 security baseline
for the v11.6 programme (ADR 0099). Every result below was run and its output
read. Where a check was not run, this says so.

## Summary

| Check | Tool / command | Result |
|---|---|---|
| Secret scan | `./scripts/check-secrets.sh` | `secret scan: nothing found` |
| Dependency policy | `./scripts/check-dependencies.sh` | `11 third-party package(s), all permitted` |
| Advisory / licence / ban policy | CI `cargo-deny`, `security audit` (run 36168396965) | pass |
| Container vulnerability scan | CI `vulnerability scan` (run 36168396965) | pass |
| SAST — Rust, Terraform, TypeScript/React, GitHub Actions, Dockerfiles, secrets | `semgrep 1.178.0 --metrics=off` with `p/rust p/terraform p/typescript p/react p/secrets p/github-actions p/dockerfile` over 1,482 files | 72 findings: **0 real high/critical**, 2 false-positive ERRORs, 54 WARNINGs, 16 INFO; 34 parse errors (below) |
| IaC static validation | `terraform fmt -check -recursive`, `terraform validate` (local 1.15.8; CI pins 1.9.8) | clean; `Success! The configuration is valid.` |

## Semgrep findings, triaged

| Severity | Rule | Count | Verdict |
|---|---|---|---|
| ERROR | `detected-google-gcm-service-account` at `scripts/check-secrets.sh:15` | 1 | **False positive.** The line is the secret scanner's own detection pattern for service-account JSON. |
| ERROR | `react-insecure-request` at `frontend/portal/src/lib/server/google-credentials.ts:79` | 1 | **False positive.** A server-side fetch to the GCE metadata server, which only speaks plain HTTP, guarded by the `Metadata-Flavor` header. It is not browser code. |
| WARNING | `github-actions-mutable-action-tag` | 45 | **Real, medium.** Actions are referenced by movable tags (`actions/checkout@v4` and similar) in `ci.yml`, `deploy.yml`, `image.yml` and `infra.yml`. v2.1 §28 lists "mutable latest tags" as an anti-pattern, and the repository already pins container images by digest for the same reason. Work item: pin every action to a full commit SHA. |
| WARNING | `gcp-cloud-storage-logging` | 9 | **Real, low.** Buckets in `modules/{ai,cloudrun,data,egress-proxy,evidence,image-bake,public-edge}` declare no access-log sink. v2.1 §15 asks for Cloud Audit Logs data-access evidence. Work item: decide per bucket, since the evidence bucket matters most. |
| INFO | `temp-dir` | 15 | Test code under `apps/*` and `qip-core::secret` tests using `std::env::temp_dir()`. Not a production path. No action. |
| INFO | `args` at `qip-cli/src/main.rs:53` | 1 | The CLI reads its own arguments. No action. |

The 34 Semgrep "errors" are not findings, and none of them is in Rust:
- 30 are `PartialParsing` of shell snippets embedded in workflow YAML, mostly
  `deploy.yml` around lines 412 and 445;
- 2 are internal matching errors on `.github/workflows/vendor.yml`;
- 2 are partial parses of `.tsx` files.

**Those snippets were analysed only partially**, so the GitHub Actions result
covers those workflows incompletely. The Rust ruleset did run on the Rust
files: the `temp-dir` and `args` findings come from it.

## Not run, and why

- **Claude Security / a model-led security review.** This is a baseline of
  deterministic tools. The `security-engineer` agent reviews each change as
  it lands, which is where model judgement pays for itself.
- **Trivy locally.** The CI `vulnerability scan` job runs it on every PR, and
  it passed on run 36168396965.
- **Cloud posture (Security Command Center, IAM analyser).** No local cloud
  credential exists by decision (the gcloud login was deferred on 2026-09-25),
  and the dev project's billing is disabled.

## Standing security facts the programme must not regress

- Paper trading is enforced at three layers (`01-security-and-safety.md`), and
  ADR 0099 carries them forward unchanged.
- There are no service-account keys. GitHub reaches GCP through Workload
  Identity Federation only.
- Secrets reach processes as files. The dependency surface is 11 packages
  (serde and serde_json plus their transitive closure).
