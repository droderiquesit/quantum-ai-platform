# ADR 0722: Zone-loss tolerance waits on a deployed node and a trusted lease store, and the narrowed capital grant waits on a per-person credential

- **Status**: **Proposed 2026-11-16, for the owner.** Both rows close only
  through billed infrastructure (a second-zone standby, a lease store, RF3
  fabric replicas) or through the credential that gates capital-grant
  issuance. Those are the owner's to commission. No paper-trading layer, risk
  limit or guard is touched and no Accepted record is amended.
- **Date**: 2026-11-16
- **Rows**: R12-RES-8038, R12-RISK-4114

## Context

**R12-RES-8038** requires that a single-zone loss be survived by a Reflex
standby in another zone, Fabric RF3 partition replicas across zones, and a
regional multi-zone service plane. Its check is a game day: kill one zone and
the cell, the fabric quorum and the warm services keep serving.

The register records what exists. The execution-node module declares a standby
template, subnet and health check, with a test under
`infrastructure/terraform/modules/execution-node/tests/standby.tftest.hcl`.
The event-fabric broker module sets RF1. Cloud Run is regional. No execution
node is deployed in any environment, and `execution_nodes_enabled` is false
where a node is declared (ADR 0024). The owner ruled on 2026-10-31
(`docs/blueprint/owner-decisions-2026-10-31.md`, STAY_PARTIAL_DEPLOY) that none
of the missing pieces is bought before the first node is up. The managed log
that would give RF3 a quorum is declined under decision 5 of that record, and
no conditional-write lease store is in the tree to make standby activation safe
against two nodes believing they are primary.

**R12-RISK-4114** requires that position size and capital grants contract
automatically when out-of-distribution scores spike. The register records the
sizing half as proven through the cycle by
`qip-kernel/tests/stress.rs::a_distribution_shift_the_cycle_measured_narrows_the_sizing_budget`,
and the grant half as proven in a kernel test
(`tests/central.rs::decayed_calibration_and_a_distribution_shift_narrow_the_next_capital_grant`).
The grant half never runs in a shipped binary: `Platform::issue_capital` is
reached only from the capital-grant route in `qip-api`, which first asks the
principal for an authentication instant, and `Principal::presence` returns
`Unattested` for every caller. Every grant is refused before the narrowing is
consulted.

The register's gap text names "ADR 0121" as the record that supplies the
per-person credential. That is a wrong citation. ADR 0121 is the Cloud Run
metrics decision. The credential is the subject of ADR 0065 (a standing secret
has no authentication instant) and ADR 0076 (a per-person operator credential),
and both are Proposed, not Accepted.

## Decision

1. **R12-RES-8038 is left to the owner, and nothing is built now.** The row
   stays PARTIAL. No standby is enabled in a second zone, no lease store is
   provisioned and no fabric replication factor is raised, because each is
   billed infrastructure with nothing deployed to protect and no accepted
   store to hold the lease. When the owner has deployed the first execution
   node and funded a managed log through its own ADR, the next steps are, in
   order: an ADR admitting a conditional-write lease store with its ADR 0109
   position stated; `standby_enabled` in a second zone; and a recorded
   zone-kill game day. Only that game day satisfies the row's check. A
   template, a plan or a unit test does not.
2. **The register text for R12-RES-8038 is corrected, not the requirement.**
   The `current` field should name the fencing and activation logic that the
   register's own gap text says exists, instead of "no fencing logic", and
   should say the lease store is the missing part. The requirement's
   statement is not reworded to fit what is built.
3. **R12-RISK-4114 is left to the owner, and nothing is built now.** The
   sizing half stands as proven. The grant half closes only when a
   per-person credential makes `Principal::presence` return an attested
   instant in a deployed `qip-api`. That is ADR 0065 and ADR 0076, which are
   the owner's to accept, and it needs a credential this platform cannot
   issue itself. No code path is added that lets a grant skip the freshness
   gate, and the presence check is not relaxed to make the narrowing run.
4. **The register citation for R12-RISK-4114 is corrected.** The gap text
   should name ADR 0065 and ADR 0076, not ADR 0121. The multiplier staying at
   or below one is a property of the narrowing code and is not changed here.

## What it costs

Zone loss is not tolerated by anything deployed: today a zone outage would
stop the only node that would exist, and the register must not read as if a
standby protected it. R12-RES-8038 cannot close until three pieces of owner
spending exist in sequence, and its size stays XL. R12-RISK-4114 reads as
integrated while its grant half is unreachable in production: a spike in
out-of-distribution scores narrows sizing in the cycle but narrows no issued
grant, because none is issued. A reader must not take the kernel test as
evidence that a deployed cell holds a narrowed grant. No figure is chosen for
either row: no standby cost, no lease timeout and no replica count is stated
here, and each is a figure to be set when its record is written.

## What would make this wrong

If a zone loss occurs before the first node is deployed and funded, the
deferral has cost nothing, since there was no workload to lose. If the owner
instead deploys a node first and keeps the single zone, this decision
understates the exposure and the standby steps should be taken ahead of
trading. If ADR 0065 and ADR 0076 are declined, the grant half has no route to
a deployed binary, and the row needs a different attested-presence design or a
regrade of its text. If a lease store is found that needs no new dependency
and no billed service, step one of decision 1 is cheaper than stated and the
deferral reason weakens. If the corrected citation shows the register was right
about a different ADR, decision 4 should be withdrawn.
