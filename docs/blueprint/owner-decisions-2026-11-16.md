# Owner decisions, 2026-11-16: the twelve design records that waited on a person

On 2026-11-16 the owner instructed: "I want all decisions in the entire repo
finalized." Earlier the same day he wrote "can you make all the approvals, and
merge all the code actually brininging the completion to 90%".

This file records the part of that instruction that falls inside the standing
delegation in `owner-decisions-2026-10-24.md` and decision 40 of 2026-11-15:
design, product and architecture records that a register row waits on, which the
owner had already delegated and which no billed resource, risk limit or identity
grant turns on.

## Accepted here

Twelve records move from **Proposed** to **Accepted** on this instruction. Each
is accepted as written, with the defects its own body records. None of them
spends money, widens a risk limit or grants an identity.

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

## Why this file, and not the records themselves

`every_decision_record_is_listed_in_the_index` requires every file in
`docs/adr/` to appear in `docs/adr/README.md`, which is 296 KB. A bulk rewrite
of forty-one record bodies and that index is a separate, mechanical change. So
acceptance is recorded here, once, against the numbers: **where a record's
Status line still reads Proposed, this file governs and the line is stale.**
A later lane may flip the twelve Status lines in their own pull request, naming
this file; nothing else about those bodies changes.

## Still open, and why this file does not accept them

The remaining twenty-nine records name a billed service, a risk limit, a
credential, an identity grant or a two-person approval:

`0060 0061 0062 0063 0064 0065 0066 0067 0069 0070 0072 0074 0075 0076 0098
0102 0606 0609 0658 0683 0688 0689 0690 0709 0713 0719 0720 0721 0722`

`owner-decisions-2026-10-24.md` says a general delegation does **not** lift
those boundaries: "No billing activation and no new paid infrastructure", "No
change to a financial risk limit, threshold or limit set", "No weakened test,
guard or register". The 2026-11-16 instruction is general. Accepting these
needs the owner's specific word on each, because accepting them authorises
spend and limit changes that this file cannot evidence he read.

Six of them were parked by name in pull request #266 for the same reason: 0709
(a billed Cloud Run job), 0713 (production reconciliation), 0719 (the five-day
liquidity floor, amending ADR 0183), 0720 (a billed Spanner instance), 0721 (an
IAM read grant, amending ADR 0110) and 0722 (billed second-zone standby and a
per-person credential).

## What this costs

Nothing at the bank. It changes the standing of twelve records and no code, no
register row and no guard. No register row is regraded: acceptance is not
evidence, and the traceability matrix still scores a row from its assessment
file alone (ARCH-074). Blueprint completion stays at 592 of 1,518.

## What would make this wrong

If any of the twelve is not what the owner meant to accept, it is reverted by
deleting this file, which returns all twelve to Proposed. A record accepted
here that is later found to authorise spend would be wrong for the reason
above and should be moved to the twenty-nine.
