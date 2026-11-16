# Owner decisions, 2026-11-16: all forty-one records that waited on a person

On 2026-11-16 the owner instructed, in two messages:

> "I want all decisions in the entire repo finalized."

> "Accept all 29 remaining decisions including the billed infrastructure,
> risk-limit and identity ones, and record it as my instruction."

The second message is the specific authorisation that `owner-decisions-2026-10-24.md`
requires for the boundaries a general delegation does not lift: billing, paid
infrastructure, a financial risk limit, and an identity grant. This file records
both instructions against the forty-one records that were in **Proposed**.

## Accepted here: twelve design records

Accepted as written, with the defects their own bodies record.

| Record | What it decides |
|---|---|
| 0058 | The market factor is the equal-weighted return of the committed universe |
| 0059 | A candidate's edge is re-measured against reacting counterparties |
| 0068 | A cycle is assigned one of eight execution paths, or refused |
| 0073 | A region gates its own side of a mirror; the band never arrives on the wire |
| 0107 | Blueprint v12.0 adoption and the GCP v3.0 platform architecture |
| 0608 | A synthetic world declares typed drivers; only feed-backed kinds reach market-facing readers |
| 0612 | A cell records the two regional episode kinds it can name without a figure |
| 0613 | The edge node bootstraps the fabric endpoint from its SRV record, configured address last |
| 0614 | A model manifest names a model by a fixed-length SHA-256 |
| 0622 | An observed world record carries the fabric's belief weight into the deep brain |
| 0637 | A challenge a second brain journals is answered by the LEARN stage from the retained dossier |
| 0682 | The ledger daemon adopts the shared telemetry drain |

## Accepted here: twenty-nine records that name money, a limit or an identity

Accepted on the owner's second message, which is the specific authorisation the
2026-10-24 file requires. Each is accepted as written.

| Record | What it decides |
|---|---|
| 0060 | A source is probed only where a reviewed egress route exists; no crawler is built |
| 0061 | Rule regret is a proposal, a defence or a dormancy finding; only a signed file moves a bound |
| 0062 | A venue is withdrawn on feasibility evidence and reinstated only by two signatures |
| 0063 | A sizing cap is the second consequence of counterfactual scores; a larger-size finding is only a proposal |
| 0064 | A family's funding standing is measured; the allocator's weights are not revised |
| 0065 | A standing secret has no authentication instant; a freshness gate refuses one |
| 0066 | A regime narrows an allocation and never favours it |
| 0067 | A quote is a priced intent with no path to a venue |
| 0069 | An infrastructure gate is proven by a plan; capability nothing consumes is not declared |
| 0070 | A venue is promoted to the simulator and no further; the MEV estimate rides the minimum notional |
| 0072 | The collateral graph refuses the arrangements it cannot cost |
| 0074 | A streaming estimator declares a bound that can be exceeded, and is implemented in-tree |
| 0075 | The capital-issuance route is authorised in shape and refused in fact |
| 0076 | A per-person operator identity is a per-person credential; the two-person rule waits on one |
| 0098 | The development factory runs on the owner's desktop; the GCP worker fabric waits |
| 0102 | The fleet is up to forty single-packet Cloud Run Job tasks on Vertex AI under a 25 USD/day ceiling |
| 0606 | A deep-brain distillate reaches the policy issuer only inside its two-person signed pack |
| 0609 | A world/market mismatch has one paper-only consumer per kind, charged to a budget and journaled |
| 0658 | An event contract is a fully-funded payoff of one unit on the simulated exchange only |
| 0683 | The six data-plane objectives are declared together; the unbuilt four say so |
| 0688 | The symbolic veto reads contract-expiry entailment as a hard stop |
| 0689 | Four risk rows of lane 44 are not built, and the decision each needs |
| 0690 | Basis and gap are measured at book level; an exposure with no stated bound is recorded as unbounded |
| 0709 | Batch reprocessing is a Rust Cloud Run Job; the catalog, graph and wide-column stores stay unadmitted |
| 0713 | Prod reconciliation and a task-scoped agent identity wait on the owner; Config Sync and Argo Rollouts stay declined |
| 0719 | A private or illiquid position enters by its own route; the five-day liquidity floor is not relaxed for it |
| 0720 | The fabric SDK keeps thread-backed subscriptions; the Spanner ledger waits on a measured load and a ceiling |
| 0721 | A node may stage a model pack from the existing bucket and refuses a bad signature before any state changes |
| 0722 | Zone-loss tolerance waits on a deployed node and a trusted lease store |

## What acceptance does, and does not, do

Acceptance makes a record the standing decision. It does not build, spend or
deploy anything, and it does not answer a question a record itself leaves open.

- **No billing change.** Decision 39 stands: billing is paused and nothing is
deployed. An agent may not dispatch `infra.yml` or `deploy.yml`; that stays the
owner's. The records that name a billed resource (0102, 0709, 0720, 0722) name
it as unadmitted until the owner funds and enables it, and 0102's 25 USD/day
ceiling is the ceiling in `DECISIONS.md`, unchanged.
- **No risk limit is relaxed.** 0070 keeps the ladder's refusals; 0719 keeps the
five-day liquidity floor; 0063, 0064 and 0074 keep their one-name and
measurement-only limits. Accepting them fixes the decision, and the decision is
the narrow one in every case.
- **No identity is issued.** 0065, 0076, 0721 and 0722 each record that the
credential or grant they would need does not exist and is the owner's to apply.
- **No register row is regraded.** Acceptance is not evidence: the traceability
matrix scores a row from its assessment file alone (ARCH-074). Blueprint
completion stays at 592 of 1,518.
- **The paper boundary is untouched.** ADR 0003, the Terraform refusal of the
live rungs, `AutonomyLevel::deployable` and the paper-only `Cell` constructors
are not amended by any of the forty-one.

## Why this file, and not forty-one edited bodies

`every_decision_record_is_listed_in_the_index` requires every file in `docs/adr/`
to appear in `docs/adr/README.md`, which is 296 KB. Acceptance is therefore
recorded here, once, against the numbers: **where a record's Status line still
reads Proposed, this file governs and the line is stale.** A later lane may flip
the forty-one Status lines in one mechanical pull request naming this file;
nothing else about those bodies changes.

## What this costs

Nothing at the bank today, and no code. It changes the standing of forty-one
records. The cost that is real is the one each record already states in its own
section: the capabilities each declines to build, and the rows that stay PARTIAL
because of it.

## What would make this wrong

Deleting this file returns all forty-one records to Proposed. If the owner did
not mean to accept a specific record, removing its row here and restoring that
record's Status line reverts it alone, and the rest stand.
