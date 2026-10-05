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
- Applied 2026-10-04 in `algorik-platform-dev`: Artifact Registry repository `fleet`, bucket `algorik-platform-dev-fleet`, service account `fleet-worker` with `aiplatform.user` and object access on its own bucket only (`Apply complete! Resources: 5 added, 0 changed, 0 destroyed`).
- Not yet: the Cloud Run Job (needs the worker image, being built), so **no fleet agent has run a packet**.
- Models admitted by real calls: `google/gemini-2.5-flash-lite`, `qwen/qwen3-coder-480b-a35b-instruct-maas`, `qwen/qwen3-235b-a22b-instruct-2507-maas`. No free tier exists on Vertex AI.
- Cap is 40 concurrent single-packet tasks; 25 USD/day ceiling; nothing under risk, execution, capital, compliance or edge may be sent (policy gate 6).

## Blocked
- SMS: no Twilio credentials, no mail agent. See DECISIONS.md.
- Platform dev deployment: not applied. `scripts/bootstrap-deploy.sh dev` is refused by the session's permission classifier; David must run it locally from this worktree (DECISIONS.md).
- SMS: `scripts/send-sms.py` built; needs HERMES_SMTP_USER and a password file.

## Next three actions
1. Re-review revised ADR 0101.
2. Plan the dev Terraform against `algorik-platform-dev` (show the plan, then apply).
3. Open the ready roadmap items by stream (see docs/MASTER_ROADMAP.md).
