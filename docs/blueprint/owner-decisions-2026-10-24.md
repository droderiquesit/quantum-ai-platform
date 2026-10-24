# Owner decisions, 2026-10-24: the remaining code rows

On 2026-10-24 the owner delegated every remaining decision in the session:
"Make all decisions for me , I want the end result to have a full system".
Earlier the same day the owner had also said "I gove you full authority fix
it all please and move forward. Do everything you can to close all open items
before you deploy". On the GCP cost they said "If all are done and deployment
is ready and will work we can work on that but I want that last because that
has cost". On real venue orders they said "Place order for will be real once
paper works".

This file records how that delegation is used, so that a later reader can
tell a decision taken under it from one taken by the owner directly. Each
decision it covers is written into an ADR, or into the register row's
`notes`, naming this file.

## What the delegation covers

- **Design, product and ADR decisions** that a register row waits on. The
  call is the recommended one for a platform that is strictly paper trading
  today and meant to become real-money and global-scale. Each one is a new
  ADR, numbered from a block the parent session allocates to each lane so
  that parallel lanes cannot collide (see
  `.claude/rules/architecture/00-boundaries.md`).
- **Default-path wiring for paper features.** A capability built behind an
  opt-in that no committed configuration sets may be switched on by default,
  where it is paper-only and fails closed. "Fails closed" means the
  composition root refuses an invalid configuration rather than running
  without it.
- **Protected-path changes**, applied from the patches lanes left under
  `docs/blueprint/lanes/`. They ship in a pull request, and CI's
  `agent boundary` check still requires the owner to approve that pull
  request's head. Approving and merging it is the owner's sign-off.

## What it does not cover

These boundaries were set by the owner, as explicit-authorization exceptions,
and by the repository's own rules. A general delegation does not lift them.

- **No live order and no real capital.** ADR 0003 stands, as do the three
  paper-trading layers: Terraform's refusal of the live rungs,
  `AutonomyLevel::deployable` in the composition roots, and the edge `Cell`
  constructors. The ledger's refusal of `simulated=false` (ADR 0106) stands
  too. A row that needs live capital stays BLOCKED.
- **No billing activation and no new paid infrastructure.** Deployment comes
  last, at the owner's instruction. It is prepared to the point where the
  owner enables billing and dispatches `infra.yml` and `deploy.yml`, because
  agents may not dispatch either workflow.
- **No change to a financial risk limit, threshold or limit set.**
- **No weakened test, guard or register.** A guard that refuses a change is
  answered by changing the code, or by an ADR that supersedes the decision
  the guard holds. It is never answered by editing the guard to admit the
  change.
