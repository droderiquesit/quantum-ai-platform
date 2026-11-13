# ADR 0393: The ledger's Spanner instance is nam3 with its default leader at us-east4, the write origin, and its data stays in the United States until a residency review

- **Status**: **Accepted 2026-11-12 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session fw-d-01 writes
  it. It follows the owner's ruling of 2026-10-31 on R12-LEDGER-8136
  (ADR_THEN_IMPLEMENT), which named this decision's title. The limits of the
  delegation hold:
  - it sets no figure: no processing-unit bound, edition or threshold moves;
  - it narrows a placement and adds a refusal, and relaxes nothing;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency, no provider and no billed service: Spanner stays
    behind `enable_spanner`, which is false in every committed environment,
    so no plan of any environment changes;
  - it edits no protected path.
- **Date**: 2026-11-12
- **Amends**: ADR 0113 decision 1, the **Placement** clause. That clause
  named the `nam-eur-asea` declaration "the candidate" and left the
  `default_leader` to "the change that applies it". This record replaces
  the candidate and sets the leader. The rest of the clause stands: the
  ledger lives in a multi-region instance, the fabric's tables live in
  regional ones, and the edition is Enterprise Plus.
- **Rows**: R12-LEDGER-8136.

## Context

R12-LEDGER-8136 asks that the Spanner configuration and its leader region be
chosen by write origin and legal residency, and that the plan set
`default_leader`. `modules/data/main.tf` declared `google_spanner_instance.global`
on `nam-eur-asea` and set no leader. Spanner therefore picked the leader, and
the ledger's balances were replicated across North America, Europe and Asia,
although no one had decided they could be held in Europe or Asia.

The facts the choice rests on:

- `qip-ledgerd` is the ledger's only writer (ADR 0113 decision 1, ADR 0100).
  It runs in the environment's `region`, so that region is the write origin.
- A read-write transaction commits through the leader region. A leader on
  another continent adds an ocean round trip to every commit.
- The ledger holds simulated paper balances and no customer data, so no
  foreign residency rule binds it today (the owner's ruling).
- `dev`, the only environment that has been applied, is in `us-east4`.
  `test`, `stage` and `prod` are declared in `europe-west2`, and Spanner is
  off in all four.

## Decision

1. **The ledger written from `us-east4` is placed on `nam3`, with the
   database's `default_leader` set to `us-east4`.** `nam3` is read-write in
   us-east4 and us-east1, with a witness in us-central1. If Virginia is
   lost, the leader fails over to South Carolina, and the data stays in the
   United States.
2. **The placement is keyed by the writer's region.** It lives in
   `local.spanner_placements` in `modules/data/main.tf`. The instance reads
   its `config` from that entry. The database sets its leader by DDL
   (`ALTER DATABASE ... SET OPTIONS (default_leader = ...)`), because
   provider `~> 6.12` has no attribute for it.
3. **A region with no decided placement is refused, not defaulted.** If
   `enable_spanner = true` in a region that is not in the map, the plan
   fails a `lifecycle.precondition` that names this record. A default would
   choose a residency for data nobody reviewed.
4. **Data stays in the United States until a residency review says
   otherwise.** Turning Spanner on in `europe-west2`, or in any other region,
   takes two steps: a new map entry, and the residency review that justifies
   placing the book there. Both are recorded in a record that amends this
   one.

## Evidence

- `infrastructure/terraform/modules/data/tests/spanner_placement.tftest.hcl`
  runs three plans. In us-east4, the plan gives config `nam3` and a DDL
  leader of `us-east4`. In europe-west2 with Spanner on, the plan is refused
  (`expect_failures` on the instance). In europe-west2 with Spanner off, the
  plan is admitted. `terraform test` in `modules/data`: 31 passed.
- `qip-acceptance`'s `infrastructure` suite,
  `the_ledger_spanner_instance_is_enterprise_plus_in_a_multi_region_configuration`,
  now requires two things: the instance reads the placement, and every
  configuration in the map is multi-region.

## What it costs

- A ledger written from `test`, `stage` or `prod` in `europe-west2` cannot
  turn Spanner on until somebody decides a placement and reviews residency.
  That refusal is deliberate, and it holds nobody up today, because Spanner
  is off everywhere.
- `nam3` gives up the third-continent replica that `nam-eur-asea` had, so a
  loss of every US region would take the ledger down. The book is
  simulated paper and can be rebuilt from the event log, and that is
  accepted.
- The leader is set by DDL and not by an attribute. A later change to the
  leader appends a statement and does not edit one in place.

## What would make this wrong

- A residency review that finds the paper book must sit outside the United
  States, or a writer that moves out of us-east4. Either one needs a new
  entry in the map and a record that amends this one.
- Customer or client data reaching the ledger. That would bring a residency
  rule into force, and this record assumed none applies.
