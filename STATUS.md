# STATUS

Last updated: 2026-10-04 (checkpoint 1). Project: Algorik (blueprint v12.0 title).

Phase 0 complete bar Review Board re-check of the revised ADR 0101; Phase 1 foundation started.
Branch: `docs/hermes-phase-0`, worktree `.claude/worktrees/hermes-phase-0`.

## Measured so far
- Register, 2026-10-05 after three rounds of requirement lanes: 328 complete (20.5%), 653 partial, 374 missing, 166 blocked, 80 unscored of 1601. Most closures are library-level (`integrated=false`).
- Integrated tree (`db97c010`): `cargo test --workspace --no-fail-fast` exit 0, 588 `test result:` lines, 7131 passed, 0 failed, 0 ignored; clippy 0 warnings; fmt clean.
- Unmerged: REFLEX lane (edge-cell conflicts, being resolved), RISK lane (its new guard RISK-001 refuses the CAPITAL lane's `rebalance.rs` as an unnamed TransferIntent originator; needs an owner decision, not a guard edit), six round 3 worktrees cut off mid-work by a session limit (uncommitted, kept).
- Blueprint of record in direction: v12.0 and GCP v3.0 (ADR 0101). 1,601 requirements, 31 domains.
- Nothing of the platform itself is deployed; no market data is ingested from a real source; the portal has not been run this session.

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
