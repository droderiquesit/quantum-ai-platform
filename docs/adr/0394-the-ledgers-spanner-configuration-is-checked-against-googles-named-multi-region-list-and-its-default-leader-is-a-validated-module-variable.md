# ADR 0394: The ledger's Spanner configuration is checked against Google's named multi-region list, and its default leader is a validated module variable

- **Status**: **Accepted 2026-11-12 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session fix-a-01 writes
  it, on the number the orchestrator gave it. It completes the closure of
  the owner's 2026-10-31 ruling on R12-LEDGER-8136
  (`docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`), whose
  decision text adds the guard change below and whose closure names the
  leader variable. The limits of the delegation hold:
  - it sets no figure: no processing-unit bound, edition or threshold moves;
  - it narrows what a guard admits and adds a validation, and relaxes
    nothing;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency, no provider and no billed service:
    `enable_spanner` stays false in every committed environment;
  - it edits no protected path.
- **Date**: 2026-11-12
- **Amends**: ADR 0393, two clauses. Its placement table's `default_leader`
  for `us-east4`, which ADR 0393 wrote as the literal `"us-east4"`, is now
  `var.spanner_default_leader`, defaulting to the same value. And its
  statement that every placement's configuration is multi-region, which the
  acceptance suite held only as "not `regional-*`", is now held against a
  named list. ADR 0393's decision — `nam3`, leader at `us-east4`, data in
  the United States, an undecided region refused at plan — stands unchanged.
- **Rows**: R12-LEDGER-8136.

## Context

An independent check of ADR 0393's grade found two parts of the owner's
closure unmet.

1. The ruling's decision text says: replace the hyphen check in
   `infrastructure.rs::the_ledger_spanner_instance_is_enterprise_plus_in_a_multi_region_configuration`
   with an explicit list of Google multi-region configuration names, checked
   on Google's configuration page, that admits `nam3` and still refuses
   `regional-*` and any single region. ADR 0393's session dropped the hyphen
   check, since `nam3` has none, and kept only `!starts_with("regional-")`.
   That admits a bare region such as `us-east4`, every `dual-region-*`
   configuration, and any misspelling — including `nam-eur-asea`, the string
   the ledger was declared on before ADR 0393, which Google never published
   (billed-plan finding 4).
2. The closure says the positions database's `default_leader` comes from a
   `modules/data` variable defaulting to `us-east4`, validated against
   `nam3`'s read-write regions. ADR 0393 wrote the leader as a literal in
   `local.spanner_placements`.

## Decision

1. **The guard is a list.** `GOOGLE_SPANNER_MULTI_REGION_CONFIGS` in
   `qip-acceptance/tests/infrastructure.rs` holds the multi-region names
   Google's instance-configuration page
   (`docs.cloud.google.com/spanner/docs/instance-configurations`) listed on
   2026-11-12: `asia1`, `asia2`, `eur3`, `eur5`, `eur6`, `eur7`, `nam3`,
   `nam6` to `nam16`, `nam22`, `nam23`, `nam24`, `nam-eur-asia1` and
   `nam-eur-asia3`. Every placement's configuration must be on it. The
   `regional-` refusal is kept beside the list, so the test refuses
   everything it refused before. The `dual-region-*` configurations are left
   off: ADR 0113 places the ledger in a multi-region instance, and a dual
   region is not one. A name Google adds later enters by a reviewed edit of
   the list, never by a pattern.
2. **The leader is a variable.** `modules/data` declares
   `spanner_default_leader`, default `us-east4`, validated to `us-east4` or
   `us-east1` — `nam3`'s two read-write regions on the same page, with
   `us-east4` Google's default leader. `us-central1`, `nam3`'s witness, is
   refused: a witness holds no leader. `local.spanner_placements["us-east4"]`
   reads it, and the positions database's DDL carries it.

## Why

- **Narrow, refuse or measure** (decision 28, first preference). A list
  admits a strict subset of what the prefix check admitted; a validation
  refuses a leader Spanner would otherwise refuse only at apply, after the
  instance had been created and billed.
- **A gate proven only to refuse is indistinguishable from one that refuses
  everything.** Each half is proven both ways: the acceptance test
  `the_multi_region_list_admits_nam3_and_refuses_a_regional_a_single_region_and_a_misspelt_name`,
  and the harness runs in `modules/data/tests/spanner_placement.tftest.hcl`
  that refuse `europe-west1` and `us-central1` and admit `us-east4` and
  `us-east1` through to the DDL.

## What it costs

- The list must be edited by hand when Google publishes a new multi-region
  configuration the ledger is to use. That is the point: the name is
  reviewed once, against the page, rather than admitted by shape.
- One more module variable. It changes no committed plan: its default is the
  literal it replaces, and `enable_spanner` is false everywhere.
- Nothing is billed and nothing is applied.

## What would make this wrong

- Google renames or withdraws a configuration on the list. The list is
  dated; recheck it against the page before relying on a name other than
  `nam3`.
- A residency review places the ledger outside the United States. That
  adds a placement entry under ADR 0393, and its leader would need its own
  validated variable or a widened validation in a new ADR.
