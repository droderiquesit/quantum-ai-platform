# STATUS

Last updated: 2026-10-04 (checkpoint 1). Project: Algorik (blueprint v12.0 title).

Phase 0 complete bar Review Board re-check of the revised ADR 0101; Phase 1 foundation started.
Branch: `docs/hermes-phase-0`, worktree `.claude/worktrees/hermes-phase-0`.

## Measured so far
- Blueprint newest: v12.0 (51 pp) plus GCP v3.0 (30 pp), already held in `docs/blueprint/source/`.
- Register: 31 requirement domains from v11.6, traceability matrix exists.
- ADR 0101 body written and linked (`24af6bae`); `docs/SYSTEM_MAP.md` and `docs/MASTER_ROADMAP.md` (93 items, 10 streams) committed. Branch pushed.
- Integrated tree, 2026-10-04: `cargo test --workspace --no-fail-fast` exit 0, 493 `test result:` lines, 6502 passed, 0 failed, 0 ignored; fmt clean; clippy 0 warnings or errors (now with `unwrap_used`/`expect_used` denied); dependency policy and secret scan pass.
- Batch 1 merged: repoint to `algorik-platform-dev`, unwrap/expect lint, lib socket guard test, opt-in Terraform billing budget, v12 requirement entries (1601 requirements rendered), two ARB reviews of ADR 0101 and its revision (19 of 20 items applied, 1 rejected).
- clippy 0 warnings, fmt clean, dependency policy and secret scan pass (docs/ops/hermes-baseline-2026-10-04.md).
- Cost: billing is disabled on `algorik-dev`; spend is 0 by construction.

## Agent fleet in GCP (ADR 0102)
- Running in `algorik-platform-dev` (2026-10-04/05): repository `fleet`, bucket `algorik-platform-dev-fleet`, service account `fleet-worker` (aiplatform.user + its own bucket only), and the Cloud Run Job `fleet` (`Apply complete! Resources: 1 added`), image `worker@sha256:18ca55ee...` pushed to the repository.
- **First packet ran end to end**: run `run-0001`, one task, `google/gemini-2.5-flash-lite`, status ok, 584 tokens in / 52 out, 80 micro-USD, output and ledger objects written to the bucket.
- Not run: a 40-packet batch (the permission classifier refused the dispatch; nothing was split to get round it). Admitted models and prices: ADR 0102 appendix.
- Fence: one packet per task, max 40 concurrent, 25 USD/day ceiling, nothing under risk, execution, capital, compliance or edge may be sent (gate 6, enforced by the shared validator).

## Blocked
- SMS: no Twilio credentials, no mail agent. See DECISIONS.md.
- Platform dev deployment: not applied. `scripts/bootstrap-deploy.sh dev` is refused by the session's permission classifier; David must run it locally from this worktree (DECISIONS.md).
- SMS: `scripts/send-sms.py` built; needs HERMES_SMTP_USER and a password file.

## Next three actions
1. Re-review revised ADR 0101.
2. Plan the dev Terraform against `algorik-platform-dev` (show the plan, then apply).
3. Open the ready roadmap items by stream (see docs/MASTER_ROADMAP.md).
