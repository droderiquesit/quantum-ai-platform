# Dev redeploy readiness

Owner runbook for bringing `algorik-qip-dev` (us-east4) back to serving. Written 2026-10-24 at `cd5ce404` (branch `ccr-d1bc1e54-kf6mr0`) from four read-only scout passes: images, path, digests, cost. No cloud API, gcloud, cargo or terraform ran for it. Anything marked UNVERIFIED stays unverified until someone runs the named check.

Re-read in the tree while writing (2026-10-24):
- `deploy.yml` dispatch options and its prod refusal step
- `infra.yml:495`
- `modules/data/main.tf` and `modules/cicd/main.tf` greps
- `promotiontask.yaml:76`, `projectconfig.yaml:15`
- `ci.yml:18-19`
- tfvars `:652`
- foundation budget
- the 7 + 7 dev digest pins
- the protected-path classes in `scripts/agent-policy.json`
- the redis digest at public.ecr.aws

Paper trading only. Nothing here adds, enables or eases a path to a live venue. `execution_nodes_enabled = false` stays. The three paper-trading layers in `.claude/rules/01-security-and-safety.md` are untouched.

## 1. State today

Project `algorik-qip-dev`, number `101249892564`, us-east4. It was torn down on 2026-09-13 (`infrastructure/CLAUDE.md`) and rebuilt by `infra.yml` runs 108–122; the last apply was run 118. `infra.yml` run 121 (`38988479925`, `apps dev`, 2026-10-17) succeeded with `bootstrapped qip-dev-control-plane; qip-dev is Synced and Healthy`. Kargo was not applied (`qip-github-app-kargo-dev holds no enabled version`), and the owner slots `qip-market-data-key-dev qip-quantum-token-dev qip-venue-credential-dev qip-github-app-argocd-dev qip-github-app-kargo-dev` are empty. Run 122 (`38988659352`, `urls dev`) listed `qip-dev-api`, `qip-dev-deepbrain`, `qip-dev-fastbrain`, `qip-dev-openobserve` and `qip-dev-portal` (`https://qip-dev-portal-lqxywiulvq-uk.a.run.app`); only the portal reads `iap-enabled annotation: true`. No `infra.yml` run came after 122, so no `suspend` or `teardown` ran through the workflow. The last good deploy is run 284 (`39181663463`, `27ac0a5`, 2026-10-19). Deploy runs 287 (`40099736039`) and 288 (`40120711664`) built and scanned, then every push failed with `failed to fetch oauth token ... us-east4-docker.pkg.dev/v2/token?... 403 Forbidden`. The cause is UNVERIFIED; it fits billing disabled, project shut down, or a lost IAM grant. Deploy run 289 (`40135764888`) says `ci concluded 'failure'; nothing is deployed.` because ci run 1263 (`40134647946`) failed job `sast / Analyze (rust)` with `static analysis refused: 14 finding(s) at or above 7 (high)`. `infrastructure/gitops/envs/dev/kustomization.yaml` pins deploy run 268 (`0053f401`, 2026-10-13). Billing state, and whether the cluster, the services and the registry contents still exist, are UNVERIFIED. Billing activation is outside the agent delegation (`docs/blueprint/owner-decisions-2026-10-24.md`).

## 2. Before billing: agent work (free)

None of these paths is in `scripts/agent-policy.json` `protected_paths`. The one exception is the new ADR in item 7, which `self_reviewable_protected_paths` admits. The work lands on `ccr-d1bc1e54-kf6mr0`, which is 45 commits ahead of main `d8cf5cc1`. Every new test is mutation-verified. Every gate runs and its output is quoted (`.claude/rules/02-change-management.md`).

1. **SAST. This blocks ci, and so blocks deploy.** There are 14 `rust/cleartext-logging` findings, all in `backend/crates/apps/qip-edge-node/`:
   - `src/event_fabric/inputs.rs:117,121,134,149`
   - `src/scheduler.rs:574,602,630,707`
   - `tests/reasoning_outage_loop.rs:681,693,698,702,710,739`

   Fix: log the error class, not the message, as `626c529b` did. Add no entry to `scripts/sast-baseline.json`: the file is protected, and an entry would be a suppression. The item is done only when ci's `sast / Analyze (rust)` reads `success`, which is known only after ci runs.

2. **BigQuery CMEK grant.** In `infrastructure/terraform/modules/data/main.tf`, dataset `research` (`:90`) sets `default_encryption_configuration` (`:111`) on the `warehouse` key. No encrypter grant for the BigQuery service agent exists in the file. Add:
   - `data "google_bigquery_default_service_account"`
   - a `google_kms_crypto_key_iam_member` granting `roles/cloudkms.cryptoKeyEncrypterDecrypter` on the warehouse key to that account
   - `depends_on` from the dataset to that member
   - a tftest pair, mutation-verified

   Without the grant, a refusal at apply is likely but UNVERIFIED.

3. **qip-infra role gaps.** In `infrastructure/terraform/modules/cicd/main.tf` `infra_roles`, nothing grants `bigquery.datasets.create`, `bigquery.datasets.setIamPolicy` or `cloudasset.feeds.*`; a grep for `bigquery|cloudasset` finds nothing. Add a custom role with exactly those permissions. The exact set has not been checked against live (UNVERIFIED). The first apply still races the grant, so owner step 3 (P1 or `scripts/bootstrap-deploy.sh dev`) covers it.

4. **Kargo promotion loop.** Why it loops:
   - `infrastructure/gitops/kargo/base/projectconfig.yaml:15` sets `autoPromotionEnabled: true` for dev.
   - The promotion's `git-commit` step pushes to `main`.
   - `ci.yml:18-19` runs on every push to `main`, with no path filter.
   - `deploy.yml` follows ci (`workflow_run`) and labels each image with its commit. That makes a new tag, new freight, and another promotion.

   Fix: add `[skip ci]` to the `git-commit` message at `infrastructure/gitops/kargo/base/promotiontask.yaml:76`. No test pins that message: add one to the `qip-acceptance` `gitops` suite and mutation-verify it. The loop is inferred from config; Kargo has never run (UNVERIFIED). How a skip-ci commit affects the next deploy's agent-boundary range is also UNVERIFIED; dev reads that verdict as advice only (`deploy_range_advisory`).

5. **Digest re-pin PR. Do this after owner step 8, not before.** Lines in `infrastructure/gitops/envs/dev/kustomization.yaml`:
   - Kargo-owned: qip-api `:23-25`, qip-fastbrain `:26-28`, qip-deepbrain `:29-31`.
   - qip-portal `:40-42` is always pinned by hand; the Kargo Warehouse does not watch it.

   Source: the owner step 8 run log. Fallback: the set deploy run 284 (`27ac0a5`) attested. Whether these are still in the registry is UNVERIFIED.
   - qip-api `sha256:f0fecaafc7b6706a4e879a75257fca87c627a35772c61b0f23e1d5ff8e949a41`
   - qip-fastbrain `sha256:59e873fe4fe23329a135ab6f125f984f9882fafe4a0a32973de7412cd4b46cb6`
   - qip-deepbrain `sha256:05e062913298534f2fe7475204b882e7095df25e557734c4abe1b903b0b81e6a`
   - qip-portal `sha256:b9080a26abd8fa75178fd86e176fe66e704e2c75514482a31e45e3d72a736642` (from the push log; signed)

   Re-pin `infrastructure/gitops/bootstrap/argocd/overlays/dev/kustomization.yaml:11-13` (`vendor/argocd-patched`) only if owner step 7 rebuilds it. Take the digest from its `::notice title=derived image ready::` line.

6. **Docs that are stale against the tree or the runs:**
   - `docs/ops/runbooks/owner-switches.md:23-24` and `docs/operations/deploying-an-edge-cell.md:120,187`: change `c3-highcpu-8` to `c4-highcpu-8`. The tfvars say C4 and refuse C3 at plan.
   - `docs/ops/runbooks/new-project-from-scratch.md`: drop "Nothing on this page has been run" and the "Known blocker" text. Runs 108–122 used project `101249892564`.
   - `docs/ops/RUNBOOK.md:63,590` say the cluster is suspended and the bootstrap never succeeded. Run 121 reached Synced and Healthy.
   - "No kustomize newer than v5.8.1": comments in `infrastructure/egress/vendored-images.txt`, `infrastructure/egress/derived-images.txt` and `infrastructure/docker/argocd-patched.Dockerfile`. False since v5.8.2 (2026-09-30). Change the comments only; `KUSTOMIZE_VERSION` stays.
   - "Newest sidecar is 1.9.2": `infrastructure/terraform/modules/observability/NOT-SCRAPED.md` and `vendored-images.txt`. Versions 1.9.3–1.9.6 exist; record the 1.9.6 status from §6.
   - Redis:
     - `vendored-images.txt:252` says "Redis 8.2.3", but the line pins `8.2.9-alpine`.
     - `infrastructure/gitops/bootstrap/argocd/SOURCE.md` lists `8.2.3-alpine` at `sha256:08ad0b1d…`.
     - Record that tag `8.2.9-alpine` now names `sha256:5f2e6a105b2a8cc721911d5d29aadb032c25fe55b43e829e3d03413753ff47c2`. The pin is by digest, so it is unaffected.
   - `vendored-images.txt`, google-cloud-cli: the comment says 583.0.0, but the line pins `585.0.0-slim` and the digest matches 585.0.0-slim.
   - `vendored-images.txt`, OpenObserve: the per-line comment says "not deployed"; the header above it says deployed.
   - `infrastructure/terraform/catalogue.tf:736,834` name `image_digests`, which was removed (`variables.tf:172`).

7. **New ADR for the derived Argo CD image.** ADR 0093 says remedy (2) "needs its own record", and none exists. Record:
   - `infrastructure/egress/derived-images.txt:35`: `vendor/argocd` becomes `vendor/argocd-patched`, tag `v3.5.2-patched`, built from `infrastructure/docker/argocd-patched.Dockerfile`
   - vendor.yml run 31's gate result
   - run 121's outcome

   Claim the number in `docs/adr/README.md` first, in its own commit, and re-read the index right before claiming. Both files are self-reviewable (`added_only_globs: docs/adr/0*.md`, `globs: docs/adr/README.md`). Do not edit ADR 0093 itself.

8. **Contingency image bumps.** Do these only if owner step 7 must run and Trivy refuses an image (see §6, GO-2026-6303 row). The x/crypto versions below come from build stamps; none of these images has been Trivy-scanned.
   - **cert-manager v1.21.3.**
     - Digests:
       - controller `sha256:9f4f96a33c7774cef24466fcd85031997531a03a8d117f151126baa8d1290dcc`
       - webhook `sha256:055ac8e9de6a0ba6763794838fe38944acc650f68ebe1b01dd8be079e580f458`
       - cainjector `sha256:083d0e906f8eb44e255fe4ff817ae54e49f9ed82e22f1011bb4e9e4f4b8a641f`
     - Release asset sha256 `c04a51295adda2b1a775146751f973e787190778c7f35180861d9678876e5c74`.
     - Controller x/crypto v0.57.0; webhook and cainjector not measured.
     - Files:
       - `vendored-images.txt` (3 lines)
       - `infrastructure/gitops/bootstrap/cert-manager/{SOURCE.md,upstream/cert-manager.yaml}`
       - `bootstrap/cert-manager/base/kustomization.yaml:69` (acmesolver `v1.21.1`)
       - `bootstrap/cert-manager/overlays/{dev,test,stage,prod}/kustomization.yaml`
       - `infrastructure/gitops/bootstrap/README.md`
   - **Config Connector operator 1.160.0** `sha256:f4b294b15f2a3c8ea2643ac4f731430d5b4878b809653d5a40ea72e8525c41f5` (x/crypto v0.55.0).
     - Files:
       - `vendored-images.txt`
       - `bootstrap/config-connector-operator/{SOURCE.md,upstream/autopilot-configconnector-operator.yaml,overlays/{dev,test,stage,prod}/kustomization.yaml}`
       - `backend/crates/tests/qip-acceptance/tests/infrastructure.rs:10051` (asserts `image: {OPERATOR_IMAGE}:1.156.0`)
       - `infrastructure/gitops/bootstrap/README.md`
       - `infrastructure/gitops/README.md`
     - Re-verify every comment that cites the v1.156.0 RunService CRD:
       - `backend/crates/tests/qip-acceptance/tests/gitops.rs:2435,5874`
       - `infrastructure/terraform/main.tf:357`
       - `.github/workflows/infra.yml:2295` (protected)
       - `infrastructure/gitops/envs/{dev,test,stage,prod}/{api,deepbrain,fastbrain}.yaml` and `envs/dev/portal.yaml`
       - `infrastructure/gitops/envs/README.md`
       - `infrastructure/terraform/modules/iap-run/{main.tf,README.md}`
     - The bundle was not fetched, and the 4 cnrm-system digests are UNVERIFIED.
   - **Kargo v1.12.4** `sha256:9541e4b630d1bd32363a09cf72d6baf25243b207106e9acbb389a84306eb42be` (kargo x/crypto v0.57.0, helm v0.55.0).
     - Files:
       - `vendored-images.txt`
       - `bootstrap/kargo/{SOURCE.md,upstream/install.yaml,overlays/{dev,test,stage,prod}/kustomization.yaml}`
       - `bootstrap/README.md`
     - The chart was not re-rendered, and its digest is UNVERIFIED. v1.11.8 (`sha256:04eabd6940822dbb2afd2929562c2f0ae711f74c1610738007509b78e65bf188`) was not measured.
   - **Argo CD helm (x/crypto v0.53.0):** no stable upstream fix exists, because 3.5.6's helm is sha256-identical. Only a derived-image rebuild of helm would move it, and none is designed.

9. **Pre-flight.** Free, and not run for this runbook:
   - `cd backend && cargo test -p qip-acceptance --no-fail-fast --test gitops --test manifest_wiring --test infrastructure --test deploy_provenance`
   - `terraform test` in each harness directory that items 2–3 touch
   - Start each binary built from main with the env from `infrastructure/gitops/envs/dev/*.yaml`. This proves the new binaries accept the committed manifests, which is the likeliest cause of an owner step 11 failure.
   - `make check`, quoting every `test result:` line.

## 3. Before billing: protected patches for the owner

Each path below is in `protected_paths`. An agent may draft the change on the fix branch, but it merges only after your approving review of the head commit. The session's permission layer refuses agent edits to `.claude/rules/**`, so make that one yourself.

**P1. `.github/workflows/infra.yml:495`, the self-grant loop.** Needed unless owner step 3 uses `scripts/bootstrap-deploy.sh dev`. The role choice is UNVERIFIED: `roles/cloudasset.owner` is broader than the feed permissions alone.
```diff
-          for role in roles/cloudkms.publicKeyViewer roles/binaryauthorization.attestorsAdmin roles/run.admin roles/dns.admin roles/iap.admin roles/identityplatform.admin roles/container.clusterAdmin; do
+          for role in roles/cloudkms.publicKeyViewer roles/binaryauthorization.attestorsAdmin roles/run.admin roles/dns.admin roles/iap.admin roles/identityplatform.admin roles/container.clusterAdmin roles/bigquery.dataOwner roles/cloudasset.owner; do
```

**P2. `infrastructure/environments/dev/terraform.tfvars`: comments only, no value change.** `gitops_enabled = true` stays, and `infrastructure/terraform/variables.tf` needs no change.
```diff
-#   https://qip-dev-portal-523718313246.us-east4.run.app
+#   https://qip-dev-portal-101249892564.us-east4.run.app
```
`:136-142` cite `docs/ops/missing-infrastructure-register.md` and `images.tfvars`, and neither exists in the tree.
```diff
-# services `deploy.yml` last moved here — the digests images.tfvars used to
-# record — are the digests envs/dev names, so Config Connector's first
+# services `deploy.yml` last moved here are the digests envs/dev names, so
+# Config Connector's first
 # reconcile acquires each service and changes nothing; that is the evidence
-# the ADR asks for and `docs/ops/missing-infrastructure-register.md` will
-# record. The endpoint's /28 sits above the management zone's /24 and below
+# the ADR asks for. The endpoint's /28 sits above the management zone's /24 and below
```

**P3. `.claude/rules/domains/infrastructure.md:38-49`, "Known gap".** Replace the section body with:
> `deploy.yml` offers `dev`, `test`, `stage` only (`workflow_dispatch` options, `deploy.yml:67-70`), and step `refuse a production deployment on every trigger` (`deploy.yml:187`) exits 1 when `TARGET_ENVIRONMENT` is `prod`, on any trigger, since `e2948eab`. `infra.yml` refuses `prod` unconditionally. Both halves of `02-change-management.md`'s claim hold.

**P4. `infrastructure/CLAUDE.md`, state paragraphs.**
- `:119-136` say "no Cloud Run service exists" and that the Argo CD image fails `vendor.yml`'s CRITICAL gate "with no upstream release that fixes it". Add after them: `vendor/argocd-patched` (`infrastructure/egress/derived-images.txt`) passed vendor.yml run 31 at 0 CRITICAL (`e3f81695`); `infra.yml` run 121 (2026-10-17) printed `qip-dev is Synced and Healthy`; run 122 listed 5 Cloud Run services.
- `:173-174`: "six digests" becomes "seven digests" and "eight controller digests" becomes "seven controller digests" (`grep -c 'digest: sha256:'` gives 7, and 2+3+1+1). The registry was recreated by runs 108–122, so the "no longer holds them" claim is unproven either way.

**P5. `.github/workflows/vendor.yml:17-20`, header comment.**
```diff
-# One image is vendored today: the Envoy egress proxy every Cloud Run service
-# that needs egress runs as a sidecar (infrastructure/egress/envoy.yaml,
-# modules/egress-proxy). The GitOps controllers this workflow used to mirror
-# left with the cluster (ADR 0024).
+# Ten images are vendored (infrastructure/egress/vendored-images.txt: envoy,
+# openobserve, the argocd base, redis, kargo, cert-manager x3, the Config
+# Connector operator, google-cloud-cli) plus one derived image,
+# vendor/argocd-patched (infrastructure/egress/derived-images.txt).
```

**P6. `.github/workflows/infra.yml:52`.**
```diff
-# `down` destroys `module.execution_node` — the C3 machines, which bill by
+# `down` destroys `module.execution_node` — the C4 machines, which bill by
```

**P7. `.github/workflows/deploy.yml:1288`, run summary.** Kargo has never been applied in dev, and the Warehouse does not watch `qip-portal`.
```diff
-            echo "The Kargo warehouse discovers them on its own poll and its promotions"
+            echo "Where Kargo is bootstrapped, its warehouse discovers the api, fastbrain"
+            echo "and deepbrain images on its own poll (qip-portal is not watched and is"
+            echo "pinned by hand in envs/<env>), and its promotions"
```

**Merge (owner approval).** Approve and merge one PR holding §2 items 1–4, 6, 7 and P1–P7.
- Evidence: ci on the merge commit shows `conclusion: success`, with every `sast / …` job `success`.
- Failure signatures: CodeQL still flags a finding, or `agent boundary` is red.
- The `workflow_run` deploy this merge triggers fails at push with the 403 while billing is off. That failure is harmless; owner step 8 re-dispatches.

## 4. Owner steps

Run these in this order. All dispatches go from `main`. `infra.yml`, `deploy.yml` and `vendor.yml` refuse agent actors (`dispatch_refused_for_agents`). Approving one step does not approve the next.

1. **Enable billing.**
   - First run `gcloud projects describe algorik-qip-dev --format='value(lifecycleState)'`.
     - `ACTIVE`: continue.
     - `DELETE_REQUESTED`: run `gcloud projects undelete algorik-qip-dev` (30-day window), then continue.
     - Project gone for good: stop and follow `docs/ops/runbooks/new-project-from-scratch.md` instead.
   - `gcloud billing projects describe algorik-qip-dev --format='value(billingEnabled)'`
   - `gcloud billing projects link algorik-qip-dev --billing-account=<BILLING_ACCOUNT_ID>`
   - Success: `billingEnabled` reads `True`.
   - Failure: non-zero exit, or `billingEnabled` still `False`.
   - Billing starts again now for the cluster (if it still exists) and for the always-on fastbrain and deepbrain. See §5.

2. **Inventory what survived (free reads).**
   ```
   gcloud container clusters list --project algorik-qip-dev --region us-east4
   gcloud run services list --project algorik-qip-dev --region us-east4
   for f in infrastructure/gitops/envs/dev/kustomization.yaml infrastructure/gitops/bootstrap/*/overlays/dev/kustomization.yaml; do
     awk '/newName:/{n=$2} /digest: sha256:/{print n"@"$2}' "$f"
   done | sort -u | while read -r ref; do
     gcloud artifacts docker images describe "$ref" >/dev/null 2>&1 && echo "present $ref" || echo "MISSING $ref"
   done
   ```
   - The `awk` extraction prints 14 refs (checked locally). The `gcloud` half has not been run.
   - Success: every ref prints `present`. Then skip step 7.
   - Failure: any `MISSING vendor/*` means step 7 is required. `MISSING` qip-api, qip-fastbrain, qip-deepbrain or qip-portal is fixed by steps 8–9.

3. **Enable the two APIs added since run 118.**
   - `gcloud services enable cloudasset.googleapis.com bigquery.googleapis.com --project algorik-qip-dev`
   - Why: `qip-infra-dev` lacks `serviceusage.serviceUsageAdmin` on purpose (`modules/cicd/main.tf:206`).
   - Success: `gcloud services list --enabled --project algorik-qip-dev | grep -E 'cloudasset|bigquery'` prints both.
   - Failure: non-zero exit, or fewer than both listed.
   - Alternative: run `scripts/bootstrap-deploy.sh dev` in Cloud Shell. It impersonates the bootstrap service account (`roles/owner`), so one apply that you read covers the APIs and the role gaps, and P1 becomes unnecessary.

4. **`infra.yml` with `environment=dev`, `action=plan`.**
   - Expect these new resources:
     - `google_project_iam_audit_config` (audit-logs.tf x3, secrets x6)
     - asset feed: key, topic, 2 feeds, service identity
     - `module.data`: warehouse key, `research` dataset, IAM
     - firewall policy, 2 rules, associations
     - `qip-dev-workloads` log bucket and sink
     - removal of the venue-credential grant
     - the cluster, only if it no longer exists
   - Success: artifact `terraform-plan-dev`.
   - Failure: a 412 state lock. Fix with `action=force-unlock lock_id=<ID>`, then plan again.

5. **Approve the plan (read it).**
   - Stop if it destroys any KMS key, secret or bucket.
   - New billable items: warehouse key and dataset (approved 2026-10-19), asset-feed key, topic and 2 feeds, the `qip-dev-workloads` bucket, the firewall policy. Prices are in §5.

6. **`infra.yml` with `environment=dev`, `action=up`.**
   - Order: grants, reclaim (expect nothing), `plan for up` (shown), `apply tfplan`, then bootstrap: Argo CD, Config Connector operator, `.status.healthy` wait of 900s, cert-manager, Kargo.
   - Success: `Apply complete! Resources: …, 0 destroyed`, artifact `terraform-applied-plan-dev`, the rollout `successfully rolled out` lines, and Kargo's webhook rollout.
   - Failure signatures (all UNVERIFIED):
     - 403 creating the asset feed or the BigQuery dataset: covered by step 3 / P1 / §2.3.
     - BigQuery CMEK refusal: §2.2.
     - `ImagePullBackOff`: the registry lost images; go to step 7.
     - Binary Authorization denial: an image is unattested.
   - Not a failure: recreating the cluster takes 30–40 min.

7. **`vendor.yml` with `environment=dev`.** Run only if step 2 or 6 showed `vendor/*` missing.
   - Success: `mirrored and attested 10 image(s).` and `::notice title=derived image ready::vendor/argocd-patched = sha256:…`.
   - Failure signatures:
     - **Expected today:** the derived Argo CD fails on CVE-2026-77459 (run 32, §6), so a rebuilt registry gets no attested `argocd-patched` until the base is bumped past 3.5.4. Fix that before this step if step 2 shows `vendor/argocd-patched` missing.
     - `TOOMANYREQUESTS: Data limit exceeded` from public.ecr.aws stops the loop at redis (run 32). A re-run later is the fix, not a code change.
     - A Trivy CRITICAL leaves the image unattested. Trivy is installed unpinned from apt (`vendor.yml:209-216`). §6 lists images that may now be refused (UNVERIFIED); the fallback is §2.8.
     - `the copy of … arrived as …; refusing to scan or sign it.`
   - A rebuilt `argocd-patched` gets a new digest, which needs the §2.5 re-pin PR and your merge.
   - The redis source digest still resolves upstream (§6), so the copy by digest should not fail on redis.

8. **`deploy.yml` with `environment=dev`.** It runs automatically on ci success of a push to `main`. If the run started by the §3 merge died on the 403, dispatch it.
   - Needs: green ci on the head SHA, every `sast /` job green, the registry reachable. The agent-boundary range verdict is advice only in dev (`deploy_range_advisory`, ADR 0124 §3).
   - Success: job `record what was pushed (dev)` prints `api|fastbrain|deepbrain: …@sha256:…`. The portal digest is in its push log as `<sha>: digest: sha256:…`.
   - Failure signatures:
     - `ci concluded 'failure'; nothing is deployed.`
     - push `failed to fetch oauth token ... 403 Forbidden`. The `registry already holds` step hides an earlier error with `2>/dev/null || true`.
     - the `a second build of the commit is the same image` mismatch
     - a Trivy CRITICAL

9. **Approve and merge the agent re-pin PR (§2.5),** using step 8's digests.
   - Success: ci green on the PR, then after step 11 Argo CD `status.sync.revision` equals the merge commit.
   - Failure: ci red on the PR.

10. **Kargo decision, then seed (optional for first serve; required for promotion).**
    - First decide: Kargo pushes straight to `main`, but `owner-switches.md` recommends require-PR with no bypass. Either let the Kargo App bypass that rule, or leave Kargo off and keep hand-pinning (§2.5).
    - If you seed: create the GitHub App `qip-kargo-dev` with `contents: write` and install it on the repo. Then run:
      `gcloud secrets versions add qip-github-app-kargo-dev --project algorik-qip-dev --data-file=app.json`
      with `app.json` = `{"app_id": "<number>", "installation_id": "<number>", "private_key": "<PEM, \n-escaped>"}`.
    - Security: `app.json` holds a private key. Delete it after the upload, and never commit it.
    - Optional: `qip-argocd-dev` (`contents: read`) into `qip-github-app-argocd-dev`. Argo CD clones anonymously today.
    - Do not seed `qip-market-data-key-dev`, `qip-quantum-token-dev` or `qip-venue-credential-dev`: no `envs/dev` RunService mounts them. The Alpaca pair is already seeded; do not re-run `action=seed`.
    - Success: step 11 prints `bootstrapped Kargo`.
    - Failure: step 11 prints the `Kargo not bootstrapped` warning.

11. **`infra.yml` with `environment=dev`, `action=apps`.**
    - Needs: step 6 healthy, the Config Connector CRDs established, digests present and attested, every mounted slot seeded.
    - Success: `qip-dev is Synced and Healthy`, then `IAP is enabled on qip-dev-portal ... (true)`, and `bootstrapped Kargo` if step 10 seeded.
    - Failure signatures:
      - `Application never synced` (900s timeout).
      - `synced but is not healthy` (3x200s). Most likely a new binary refuses the committed env (UNVERIFIED); §2.9's start check covers it.

12. **`infra.yml` with `environment=dev`, `action=grant`, `iap_member=user:<owner address>`.**
    - Success: the binding is read back.
    - Failure signature: not recorded by the scouts (UNVERIFIED).

13. **`infra.yml` with `environment=dev`, `action=urls`.**
    - Success: 5 services listed, and the portal shows `iap-enabled annotation: true`.
    - Failure: fewer than 5 services, or the annotation is not `true`.

14. **Afterwards, if the meter must stop: `infra.yml` with `environment=dev`, `action=suspend`.**
    - This is a targeted destroy of the cluster; a later plain `up` recreates it.
    - fastbrain and deepbrain (min 1) keep billing after a suspend. No workflow action the scouts recorded stops them (UNVERIFIED).

## 5. Expected cost

**All figures are estimates.** Basis: list prices from cloud.google.com pages fetched 2026-10-24, a 730 h month, tree `cd5ce404`, and assumed usage volumes. Free tiers are per billing account; if another project already uses them, take the high end.

- **Range: about $780–$960/month** ($782 low, $959 high). Fixed spend is about $780.
- **Three largest (estimates):**
  1. GKE Autopilot control-plane pods: about $392 ($390–$400). That is 19 pods, 7.5 vCPU, 28.56 GiB. About $344 of it is 13 Argo CD, Kargo and cert-manager pods with no requests, billed at Autopilot's default of 0.5 vCPU / 2 GiB / 1 GiB ephemeral ($26.50 per pod-month).
  2. `qip-dev-deepbrain`: about $280. Min 1, `cpuIdle: false`, 4 vCPU / 8Gi, plus an Envoy sidecar at 1 vCPU / 256Mi ($48.62).
  3. `qip-dev-fastbrain`: about $105. Min 1, `cpuIdle: false`, 2 vCPU / 2Gi.
- Next largest: the GKE cluster fee, $73, if the $74.40 free credit is already used elsewhere.
- `gitops_enabled=true` accounts for about $400–$490 (cluster, pods, NAT, fee, Binary Authorization for GKE). tfvars cannot switch it off: the plan refuses because the etcd key has `prevent_destroy`. The only off switch is `action=suspend`; the two brains (about $380–$385) keep billing after it.
- Budget: `infrastructure/foundation/terraform.tfvars:15` sets dev `budget_amount = 100`, thresholds `[0.5, 0.9, 1.0]` (`:22`). It only alerts. At the estimated rate it is crossed in about 4 days. Whether it is applied, and its currency, are UNVERIFIED; the cost scout read the dev monitoring budget as off.
- Cuts not needed for the redeploy (estimates):
  - Explicit 50–250m requests on Argo CD, Kargo and cert-manager: save $250–$300/month. **Done in the dev overlays (lane L9, owner decision 9 of 2026-10-25)**: 13 containers on 11 pods, 50–250m each. Not measured on a cluster. If the cluster predates GKE's burstable Autopilot minimums, Autopilot raises each pod to its older 250m floor and the saving is smaller (UNVERIFIED).
  - Unused Argo CD notifications-controller and applicationset-controller: $52.98. **Removed from the dev overlay (same decision).** The bootstrap applies without `--prune`, so a cluster that survived still runs both until a person runs `kubectl -n argocd delete deployment argocd-notifications-controller argocd-applicationset-controller`. A recreated cluster never gets them.
  - Flow logs at `INTERVAL_5_SEC` on 5 subnets.
  - Cloud Run logs stored twice (`_Default` and the `qip-dev-workloads` sink).
  - AR cleanup policies in dry run.
  - `qip-edge-node` built and scanned on every deploy ($0.26).
- No QPU spend: `QIP_QUANTUM_PROVIDER=simulated`.

## 6. Known blockers

Method: build stamps read with `go version -m` (no execution), advisory ranges from vuln.go.dev and api.osv.dev. **No Trivy run on any candidate image.** "Cleared" is a reading of build stamps unless a gate run is named.

- **Argo CD upstream** `quay.io/argoproj/argocd`. Pinned v3.5.2 `sha256:e2aadfae709d904e87f46ba4aa49601d827b3022db22cd4d03aae816a2e7097b`, used as the derived image's base only.
  - CVE-2025-68121 (GO-2026-4337; fixed in go1.24.13 / go1.25.7 / go1.26.0-rc.3): **not cleared** at stable 3.5.6 `sha256:8e5cc5bb008df8a09dba439462e58c94c55b7735888d3ebde0c25fc05e4e5335`.
  - In 3.5.6: kustomize 5.8.1 is go1.24.0 (size 12660920, unchanged). git-lfs 3.7.1 and helm 4.2.1 are sha256-identical to v3.5.2.
  - Evidence: https://raw.githubusercontent.com/argoproj/argo-cd/v3.5.6/hack/tool-versions.sh , https://vuln.go.dev/ID/GO-2026-4337.json
- **Argo CD 3.6.0-rc4** `sha256:b7bd6c54ad9d6247a2fb121989fe7f4faaeb65dc207094118bcd10c53f931369`.
  - CVE-2025-68121: cleared by build stamp (kustomize 5.8.3 go1.27.1).
  - **Not usable:** it is a pre-release, and its git-lfs 3.8.0 carries x/crypto v0.54.0, inside the GO-2026-6303 range.
  - Evidence: https://raw.githubusercontent.com/argoproj/argo-cd/v3.6.0-rc4/hack/tool-versions.sh
- **`vendor/argocd-patched`** (what dev runs), `sha256:d0c2521327609c71da266792ccf3118f39df71e74205695d518bcedebce63f49`.
  - CVE-2025-68121: **cleared by a gate result.** vendor.yml run 31 (2026-10-12) scanned it at 0 CRITICAL (`e3f81695`). Its builder is `golang@sha256:564e366a28ad1d70f460a2b97d1d299a562f08707eb0ecb24b659e5bd6c108e1` (`GOLANG_VERSION=1.25.7`).
  - **Refused since 2026-10-18 (added by lane L9 from the run log, 2026-10-24).** vendor.yml run 32 (`39087301496`, push, conclusion `failure`) re-scanned the same `d0c25213…` and the blocking gate printed `github.com/argoproj/argo-cd/v3 │ CVE-2026-77459 │ CRITICAL │ fixed │ v3.5.2+dirty │ 3.3.15, 3.4.10, 3.5.4, 3.6.0-rc2`. The finding is in Argo CD's own binary, so the remedy is a base bump to 3.5.4 or later, with the same Dockerfile and a re-pin. Run 31's attestation still exists, so the verdict step printed `every vendored image is attested, or superseded by an attested derived image.` in a failed run. The same run's mirror loop also died at redis on `TOOMANYREQUESTS: Data limit exceeded` from public.ecr.aws. ADR 0225 records both.
  - Its presence in the registry is UNVERIFIED.
  - The test, stage and prod overlays still carry the stand-in `04fbf149…`.
- **kustomize standalone.** A fix exists upstream: v5.8.2 (2026-09-30, go1.26.5) and v5.8.3 (2026-10-08, go1.27.1).
  - Not adopted: moving `KUSTOMIZE_VERSION` changes Argo CD's parsed semver.
  - Evidence: https://proxy.golang.org/sigs.k8s.io/kustomize/kustomize/v5/@v/list
- **cloud-run-gmp-sidecar.** 1.9.2 `sha256:ff1fc68871118f1032a3ce17e2b0abd703292e883989d220244330ebdf522fd1` is commented out and REFUSED.
  - CVE-2026-56854 (GO-2026-6303, fixed in x/crypto 0.55.0): **cleared at 1.9.6 by build stamp.** 1.9.6 is `sha256:14b20c7f07e00bf275c94c3df823815dd67bc55b656346e1e4f39688c35aeb87`, uploaded 2026-10-23, built go1.26.10-X:boringcrypto with x/crypto v0.55.0.
  - CVE-2026-78662 and CVE-2026-56855 (GO-2026-6354/6355, fixed in 0.56.0): **not cleared.** The 0.56.0 bar in `NOT-SCRAPED.md` is not met.
  - OSV scores both CVSS 3.1 7.5 (HIGH), so a CRITICAL-only gate might pass: UNVERIFIED.
  - Two prerequisites are also still open: the mount prerequisite, and the RunService CRD pruning `gcs` volumes.
  - The sidecar is not on the redeploy path (`metrics_collector_image_digest` is null).
  - Evidence: https://api.osv.dev/v1/vulns/GO-2026-6303 , https://us-docker.pkg.dev/v2/cloud-ops-agents-artifacts/cloud-run-gmp-sidecar/cloud-run-gmp-sidecar/tags/list
- **GO-2026-6303 in pinned images.** This is a gate inconsistency, and it is UNVERIFIED.
  - Affected (x/crypto below 0.55.0):
    - `argocd-patched`'s helm: v0.53.0
    - cert-manager controller v1.21.1: v0.53.0
    - Config Connector manager 1.156.0: v0.53.0
    - kargo v1.11.4's bundled helm: v0.54.0
  - vendor.yml run 11 rated CVE-2026-56854 CRITICAL on the sidecar. Run 31 passed the derived Argo CD at 0 CRITICAL with the same helm inside. The cause is not found; the Trivy DB date or severity source is suspected.
  - The next vendor.yml run may refuse these images. The fallback is §2.8.
- **redis** `8.2.9-alpine`, pinned `sha256:e957842a3e7962bfe3e5ab9814eab06e029a2f0d7b0f5d74178af12713b9ab4d`. No CVE blocker recorded.
  - The pinned digest still resolves at public.ecr.aws (HTTP 200, checked 2026-10-24).
  - The tag now names `sha256:5f2e6a105b2a8cc721911d5d29aadb032c25fe55b43e829e3d03413753ff47c2` (rebuilt).
- **No blocker recorded, none scanned this pass:** kargo v1.11.4, cert-manager v1.21.1, Config Connector operator 1.156.0 (all three are also in the GO-2026-6303 row), envoy `distroless-v1.38.3`, openobserve `v0.92.2`, google-cloud-cli `585.0.0-slim`.

## 7. Not verified

- Billing state, project lifecycle, and whether the cluster, the Cloud Run services and the registry contents exist.
- The cause of the deploy 403 in runs 287 and 288.
- Whether the registry holds:
  - the run 268 pins
  - the run 284 digests
  - `vendor/argocd-patched` `d0c2521…`
- The digests scout's claim that every registry digest is "stale per teardown". It rests on `infrastructure/CLAUDE.md:172-174`, written before runs 108–122 rebuilt the registry.
- Whether CodeQL clears after §2.1.
- The custom-role permission set (§2.3) and the P1 role choice.
- The BigQuery CMEK refusal at apply.
- The Kargo promotion loop, and whether any promotion has ever run.
- Whether the new binaries accept the committed `envs/dev` env. §2.9 was not run.
- Trivy, on every candidate image.
- Images not measured: the cert-manager webhook and cainjector binaries, the argocd binary (only its go.mod was read), the google-cloud-cli contents.
- Bundles and charts: the Config Connector 1.160.0 bundle and its 4 cnrm-system digests, the Kargo 1.12.4 chart.
- Compatibility: envoy 1.38 to 1.40, OpenObserve 0.92 to 1.1.
- Upstream source digests re-resolved. Only redis was checked.
- The cause of the GO-2026-6303 gate inconsistency.
- Cost inputs:
  - usage volumes, Autopilot node count, free-tier use on the account
  - the us-east4 storage price, multi-arch scan count, managed-Prometheus samples
  - the secret version count, and a possible ninth (orphan) KMS key: `infrastructure/CLAUDE.md:115` says nine keys stood outside state, while the config declares 8
- Whether the foundation budget is applied, and its currency.
- Failure signatures for `billing projects link` and `action=grant`.
- How to stop the always-on brains after `suspend`.
- No gate ran for this runbook: `cargo fmt`, clippy, `cargo test`, `check-dependencies.sh`, `check-secrets.sh`, `terraform fmt`/`validate`, `make check`.