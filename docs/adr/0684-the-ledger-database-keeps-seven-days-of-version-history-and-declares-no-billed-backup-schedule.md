# ADR 0684: The ledger database keeps seven days of version history and declares no billed backup schedule

- Status: Proposed (lane 42, wave b52; built, not compiled or planned)
- Date: 2026-11-15

## Context

R12-LEDGER-8140 asks for scheduled Spanner backups and point-in-time recovery.
The `positions` database had neither: Spanner's default version retention is one
hour, so a logical corruption noticed after lunch is permanent. An earlier draft
folded `version_retention_period` into the single DDL statement that
`tests/spanner_placement.tftest.hcl` pins; that broke the pins.

## Decision

1. `version_retention_period` is set by its own DDL statement in
   `google_spanner_database.positions`, beside the leader statement, never inside it.
2. The value is **7d, provisional**. Nobody gave a figure; seven days is Spanner's
   maximum, so the choice errs toward recoverability. A person confirms or lowers it.
3. No `google_spanner_backup_schedule` is declared. Scheduled backups are billed
   storage and need an approved retention and cost figure first. The backup-schedule
   half of the requirement is therefore **not built** (NEEDS_FIGURE).

## Consequences

PITR becomes plannable. The requirement is not met until a schedule exists.
Paper boundary untouched. Nothing here was planned or applied.
