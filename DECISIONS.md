# DECISIONS

Newest last. Each entry: decision, why, alternatives rejected, how to reverse.

## 2026-10-04 — Mission values and scope
- **Decision.** REPO is this checkout. CLOUD_PROJECT is `algorik-dev` (already in `gcloud config`). SPEND_CEILING is 25 USD/day. David waived the repo's approval rules for merges, deploys, applies, machine-wide setup and external sends; the paper-trading boundary was not waived.
- **Why.** David said "i wave all just peform the work" after the conflicts were listed. The values were left blank, so defaults and the existing gcloud project were taken.
- **Rejected.** Creating a new cloud project (touches resources this repo did not create).
- **Reverse.** Edit the table in HERMES_MISSION.md.

## 2026-10-04 — Work continues in worktree `docs/hermes-phase-0`
- **Decision.** The session landed in `.claude/worktrees/hermes-phase-0`, which already holds the v12.0 and GCP v3.0 sources and the ADR 0101 claim. Phase 0 builds on it rather than starting a second tree.
- **Why.** ADR 0099 and the 0101 claim already encode the adoption procedure; a parallel copy would be a second source of truth.
- **Reverse.** Branch is unpushed; delete it.

## 2026-10-04 — HERMES_MISSION.md is a condensed copy
- **Decision.** The committed file keeps every operating rule, phase, invariant and the Definition of Done but is shorter than the order David pasted (roles table, §3 detail and §5 detail are summarised).
- **Why.** The original was not on disk and was re-typed from the conversation.
- **Reverse.** Replace the file with the full text.

## 2026-10-04 — SMS channel is not available; status goes to STATUS.md
- **Decision.** No Twilio credentials, no mail agent (`mail`, `sendmail`, `msmtp` absent). The 30-minute SMS cannot be sent. STATUS.md is the substitute and a one-line ask is in the session.
- **Why.** §8 requires proving the channel first; it cannot be proven. Faking a send would break the nothing-fabricated rule.
- **Reverse.** Supply Twilio credentials as files or an SMTP relay and implement the sender as a script.

## 2026-10-04 — No cloud apply in Phase 0
- **Decision.** No Terraform apply or cloud call. ADR 0099 records dev billing as disabled (C8), so cloud work is blocked externally regardless of the ceiling.
- **Reverse.** Re-check `gcloud billing projects describe algorik-dev` and then plan.

## 2026-10-04 — SMS sender: `scripts/send-sms.py`, Verizon gateway only
- **Decision.** Email-to-SMS client is a stdlib Python script (smtplib, STARTTLS) sending to `5083179114@vtext.com`. David confirmed the carrier is Verizon, so the T-Mobile and AT&T gateways are dropped. Password is read from a file, never the environment.
- **Why.** No mail agent or Twilio exists here; the script is the smallest thing that works.
- **Not done.** Not sent: SMTP user and password file are not configured, so no message has gone out. Default host `smtp.comcast.net:587` is a guess from David's address, not verified.
- **Reverse.** Delete the script; set `HERMES_SMS_TO` to change the target.

## 2026-10-04 — New cloud project `algorik-platform-dev`, billing enabled
- **Decision.** On David's instruction ("Create new project rather than enabling then enable billing"), created `algorik-platform-dev` under org `droderiques-it-org`, linked billing account `012F9F-AC0200-6FDF18` (the open one), set it as the gcloud default, enabled `billingbudgets.googleapis.com`, and created a 750 USD/month budget alerting at 50/70/90/100%.
- **Why.** `algorik-dev` had billing disabled. GCP budgets are monthly at the smallest, so the 25 USD/day ceiling is 750/month by arithmetic, and a budget only alerts, it does not stop spend.
- **Not done.** `infrastructure/environments/dev/terraform.tfvars` and the files citing `algorik-dev` (WIF audience, identity store, gitops overlays) still name the old project; repointing them is a separate change. No Terraform has been applied. Labels env/service/owner/cost-center are on the project.
- **Reverse.** `gcloud billing projects unlink algorik-platform-dev`; delete the budget `0ed82b93-a9ef-4572-843f-5f6646825ae0`; `gcloud projects delete` only on David's say-so.

## 2026-10-04 — The fleet's first slice is applied; Vertex AI is enabled; three models admitted
- **Decision.** On David's instructions ("Get the fleet up", "You adding them to gcp move on it"), enabled `aiplatform`, `run`, `artifactregistry`, `iam`, `iamcredentials` and `cloudresourcemanager` on `algorik-platform-dev`; created the state bucket `gs://algorik-platform-dev-fleet-tfstate` (versioned, uniform access, public access prevented); planned and applied `infrastructure/fleet`: `Plan: 5 to add, 0 to change, 0 to destroy`, `Apply complete! Resources: 5 added`. The Job is not created yet because no worker image exists.
- **Models.** Admitted by observation (ADR 0102 appendix): `google/gemini-2.5-flash-lite` (0.10 / 0.40 USD per 1M tokens), `qwen/qwen3-coder-480b-a35b-instruct-maas` (0.22 / 1.80) and `qwen/qwen3-235b-a22b-instruct-2507-maas` (0.22 / 0.88). **There is no free tier on Vertex AI**; the fleet is low-cost, not free. Each probe call billed a few tokens.
- **Hugging Face on GPUs: not done.** David asked for the largest open models from Hugging Face deployed in GCP. Google already serves large open models per token (the two Qwen models above answered); self-hosting weights needs GPUs, an ADR (ADR 0093, `infrastructure.md`), a price and a quota nobody has observed. No Hugging Face token was read or used.
- **Fleet state bucket is its own**, not a prefix in the platform's, because the platform's bucket does not exist and the fleet's identity should not need the platform's state.
- **Reverse.** `terraform destroy` in `infrastructure/fleet` (needs David's say-so); delete the state bucket last; `gcloud services disable` for the APIs.

## 2026-10-04 — The platform's own dev deployment is still not applied
- **Fact.** `scripts/bootstrap-deploy.sh dev` was refused three times by the session's permission classifier ("Protected-Scope IaC Apply"). It grants `roles/owner` to a bootstrap account and applies the whole platform root. David must run it in a local terminal from this worktree, or add a Bash permission rule. Nothing was done to route around the refusal.

## 2026-10-05 — The fleet ran its first packet; the batch dispatch was refused
- **Decision.** Pushed the worker image to Artifact Registry, planned and applied the one-resource change creating the Cloud Run Job (`Plan: 1 to add`, `Apply complete! Resources: 1 added`), and ran one scout packet (`run-0001`): status ok, 80 micro-USD.
- **Refused.** A dispatch of 40 design-sketch packets (one per high-priority open requirement, model `qwen/qwen3-235b-a22b-instruct-2507-maas`) was denied by the session's permission classifier with no reason. Not retried in smaller pieces. The packets are not on disk.
- **Reverse.** `terraform destroy` in `infrastructure/fleet` with David's say-so; the image can be deleted from the repository.
