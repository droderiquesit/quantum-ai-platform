# ADR 0716: BigQuery reservations stay declined and managed training placement waits on a funded trainer

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15). Both decisions decline or defer; nothing is built, no billed resource is created, and no Accepted record is amended.
- **Date**: 2026-11-16
- **Rows**: R12-FINOPS-9186, R12-FINOPS-9188

## Context

R12-FINOPS-9186 requires separate research, backtest and BI reservations for BigQuery so no one job monopolises compute. Its check is that three reservations and assignments exist. The register row is MISSING: `infrastructure/terraform/modules/data/main.tf` declares no reservation. The 2026-10-31 owner ruling (STAY_PARTIAL_DEPLOY) declined reservations for now, as decision 5 of the owner decisions declines billed infrastructure that has no measured need. The warehouse has one streaming writer and no measured query load, and dev's warehouse is not yet deployed.

R12-FINOPS-9188 requires queue- and quota-aware training, Spot where interruption is safe, and reserved capacity only if justified. The register row is PARTIAL. `TrainingSpec.scheduling` carries a queue, a capacity (OnDemand or Spot) and a priority. `TrainingSpec::validate` refuses a critical job on Spot, and `LocalTrainer::fit` and both providers call it. `LocalTrainingProvider` and `VertexAiProvider` take `QueueQuotas` and refuse a submit into a full or undeclared queue. No production path submits through a provider: the deep brain fits in process and Vertex is disabled under decision 5 and R12-MODEL-9002. The second owner ruling of 2026-10-31 kept the row PARTIAL until a managed trainer is funded.

Both rows are cost controls whose subject, a shared warehouse and a managed trainer, does not yet exist in any deployment.

## Decision

1. **R12-FINOPS-9186 is declined for now.** The warehouse stays on on-demand billing. No reservation, assignment or commitment is added to `modules/data`. A reservation caps compute and bills for slots, so adding one without a measured load would spend money against no observed contention. The row reopens only by a new ADR that names the edition, the slot counts for research, backtest and BI, and a monthly ceiling. That ADR waits on the deployed dev warehouse showing the contention a reservation exists to stop. **Provisional: no slot count and no ceiling is chosen here.**
2. **R12-FINOPS-9188 keeps what exists and builds nothing further.** The spec fields, the critical-on-Spot refusal and the quota refusals stand unchanged. An in-process fit is not labelled Spot and is not routed through a quota a sequential desk can never fill, because neither is a placement. Spot placement and quota enforcement on a real path arrive with a managed trainer, and funding that trainer is left to the owner.
3. **Funding Vertex is left to the owner.** It needs real money, so it is a choice only the owner can make. When an ADR funds it with a measured need and a monthly ceiling, the trainer setting of R12-MODEL-9002 submits the deep brain's fits through `VertexAiProvider` with declared `QueueQuotas`. On that path challenger fits would run on Spot at Standard priority and baseline fits would run OnDemand. Critical retraining would stay OnDemand, since the existing refusal forbids Spot there. Reserved capacity for critical retraining is not proposed: the requirement allows it only if justified, and nothing justifies it yet. **Provisional: no quota size and no ceiling is chosen here.**
4. **Closing evidence is fixed in advance.** R12-FINOPS-9186 closes on a plan showing three reservations and their assignments, plus a plan harness. R12-FINOPS-9188 closes on a test that a submit beyond its queue's quota is refused on the funded path, mutation-checked by removing the quotas.
5. **Neither decision touches a safety control.** No risk, capital or budget limit changes, no guard or test is weakened, no order path is touched, and ADR 0150 is not engaged, since no quantum family is involved.

## What it costs

A research, backtest or BI query can still take as much on-demand compute as the project's default allows, so one runaway query is bounded only by whatever project-level quota or billing alert exists, not by a reservation. The research and backtest workloads share a warehouse today, but only one streaming writer exists. This is the accepted risk of not buying capacity before measuring.

Training has no Spot placement in any deployment. The deep brain pays no managed-trainer cost, and it also gains none of the Spot saving or the queue protection. The scheduling code is carried and tested with no production caller, so a regression in how a managed path would use it would not show until that path is funded.

## What would make this wrong

If the deployed dev warehouse shows one job starving another, the on-demand position is wrong and the reservation ADR should be written at once, not after the next cycle. If a query-cost incident occurs before dev is deployed, the reason for waiting, that no load is measured, is gone. If the deep brain's in-process fits outgrow one machine, or a critical retraining misses a deadline for lack of capacity, then the position that no managed placement is needed is wrong, and the owner's funding choice becomes urgent. If the owner funds Vertex without a ceiling, decision 3 does not apply to that funding and a new record is required.
