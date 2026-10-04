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
