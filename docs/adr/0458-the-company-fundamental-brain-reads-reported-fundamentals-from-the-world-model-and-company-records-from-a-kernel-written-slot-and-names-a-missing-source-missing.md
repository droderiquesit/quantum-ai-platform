# ADR 0458: The Company/Fundamental Brain reads reported fundamentals from the world model and company records from a kernel-written slot, and names a missing source missing

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR), session t50-16. The delegation's limits hold:
  it sets no figure the owner has not stated, changes no requirement's
  wording, adds no dependency and touches no paper-trading layer. The one
  number it introduces — the one-year window "debt due soon" is measured
  over — is a convention of credit analysis, not a limit; nothing sizes,
  vetoes or trades on it.
- **Date**: 2026-11-12
- **Rows**: R12-REASON-3013.

## Context

R12-REASON-3013: "A Company/Fundamental Brain exists as a specialist
covering: financial statements, earnings, corporate events, management
guidance, credit and capital structure." Its check: the brain "is
constructed in the production organisation and emits findings for its
scope; removing it drops its scope from coverage."

Before this record the equity analyst read price bars only and the credit
analyst read one spread. The fundamentals the kernel absorbs
(`Platform::observe` → `WorldModel::absorb_fundamental`, which records the
metric and its `<metric>_surprise` keyed by issuer entity and tagged
`world_stream:fundamental`) reached no specialist. Corporate actions are held
by the kernel (`corporate_actions_pending`) for price adjustment and read by
no agent. Management guidance and capital structure had no record type at
all.

## Decision

1. **One agent, `CompanyFundamentalAnalyst` (`company-fundamental-brain`),
   in `Organisation::standard`**, owned by investment research, with the
   standard analyst reads and nothing else: it reaches no book, no market
   and no execution history.
2. **Five scopes, each evidenced by its own fact labels** in
   `qip_investment_agents::coverage::Scope`: `FinancialStatements`
   (`statement_metrics_reported`), `Earnings` (`earnings_surprise`),
   `CorporateEvents` (`corporate_actions_pending`), `ManagementGuidance`
   (`guidance_gap`, `guidance_outstanding`), `CapitalStructure` (`net_debt`,
   `net_leverage`, `debt_due_within_year`). One scope per clause of the
   requirement, so removing a source drops exactly its clause.
3. **The issuer is the knowledge graph's `Issues` predecessor of the
   instrument**, the edge the kernel writes; the reference record's `issuer`
   field is read only where the graph holds none. No issuer is no data,
   saying so.
4. **Statements and earnings come from the world model only**, and only from
   readings tagged `world_stream:fundamental` and readable at the as-of in
   both time dimensions. A derived feature on the same entity is not a
   reported figure.
5. **Corporate events, guidance and capital structure come from a desk slot,
   `CompanyView`, behind `read_reference_data`**, which the kernel rewrites
   every REASON (`Platform::brief_the_desk_on_company`) from the corporate
   actions awaiting their ex-date. `corporate_actions_observed` is false
   until any corporate action has reached `queue_corporate_action`, so a
   platform no feed reaches reports *no source*, not *no events*.
6. **Guidance and capital structure are typed records in
   `qip_financial::company`** (`ManagementGuidance`, `CapitalStructure`,
   `DebtMaturity`) whose constructors refuse a transposed range, negative
   debt or cash, a ladder owing more than the stated total debt, and a
   publication before the balance-sheet date. Leverage without a positive
   EBITDA is absent, not infinite. Guidance is judged only against a
   reported figure whose valid time is the guided period's end.
7. **A sub-scope with no source is named in `missing_inputs`, never reported
   as zero.** No ingestion arm in the tree produces guidance or a capital
   structure, so on every deployed binary today the brain covers statements
   and earnings (and corporate events once a corporate-action feed is
   connected) and names the other two missing. That is the honest finding.
8. **The finding states no direction.** The issuer's consensus surprise
   already reaches the forecast as `fundamental_surprise` (R12-WORLD-3044);
   a vote from the same reading would be a correlated second opinion from
   one origin, the reason ADR 0147 decision 8 gives for the capital
   position.

## Consequences

- The organisation is twenty-six agents and the roster twenty-seven.
- Applied corporate actions are kept by the kernel as keys only, so the brain
  reads announced actions awaiting their ex-date and says so in a caveat.
- Closing the guidance and capital-structure sources needs an ingestion arm
  (a `SensedRecord` variant and an absorber) and a licensed source. That is
  outside this record and is the remaining gap on R12-REASON-3013.

## What would make this wrong

A reader taking the brain's absence of a guidance figure as guidance met; a
second writer of `CompanyView` other than `brief_the_desk_on_company`; or a
fundamentals path that records company figures without the
`world_stream:fundamental` tag, which this brain would then not see.
