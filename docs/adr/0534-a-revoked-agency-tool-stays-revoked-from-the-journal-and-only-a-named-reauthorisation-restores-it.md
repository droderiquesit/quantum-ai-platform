# ADR 0534: A revoked agency tool stays revoked from the journal, and only a named reauthorisation restores it

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**,
  session t5-04 (wave580). An in-repository design decision: it adds two
  records to the agency journal and makes a withdrawal outlive the policy
  line that made it. It widens no authority, adds no dependency, invents no
  figure and touches no paper-trading layer.
- **Date**: 2026-11-13
- **Rows**: R12-RES-7109 (stays PARTIAL; see "What this leaves open").

## Context

`qip agency shadow` is the shipped path that runs the agency loop.
R12-GOV-6123 gave it a revocation: `AgencyPolicy::revoked_tools`, applied to
the guard on every start. The policy file was the only record of the
withdrawal. Deleting the line readmitted the tool on the next start with no
named operator, no reason, and nothing on the journal saying it had ever been
stopped. `ToolRegistry::reauthorise(tool, operator)` and
`ActionGuard::reauthorise` existed in the library and no shipped path called
them, so the library's rule — a revoked tool comes back only on a named
operator's authority — held in tests and nowhere else.

## Decision

1. **A revocation is journaled.** The first pass that runs under a policy
   revoking a tool writes `AgencyRecord::ToolRevoked`, after `PassOpened` and
   before any tool decision, so a reader meets the withdrawal before the
   refusals it causes. A revocation the journal already holds is not written
   again.
2. **A revocation stands from the journal.** `journal::resume` folds the
   latest `ToolRevoked` / `ToolReauthorised` per tool into
   `Resumed::standing`, and `Session::open` re-revokes every tool the journal
   holds withdrawn before it reads the policy. Deleting a name from
   `revoked_tools` readmits nothing. A tool withdrawn on the journal and since
   dropped from the registrations is skipped: an unregistered tool is refused
   anyway.
3. **Only a reauthorisation record restores it.** `AgencyPolicy::reauthorisations`
   holds `{tool, operator, reason}` entries. Each goes through
   `ActionGuard::reauthorise`, which refuses a blank operator and a tool that
   is not revoked. The binary also refuses a blank reason and a policy that
   both revokes and reauthorises one tool. An accepted entry is journaled as
   `AgencyRecord::ToolReauthorised` beside the pass's tool decisions. An entry
   the journal already holds verbatim after the tool's last revocation is the
   same decision read again and is neither re-applied nor re-journaled.
4. **Every refusal stops the process before a pass runs.** A start that
   would journal a reauthorisation it cannot honour journals nothing.

## Evidence

`qip-cli/tests/agency.rs::deleting_a_revocation_from_the_policy_readmits_nothing_and_only_a_named_reauthorisation_restores_the_tool`
drives the shipped binary over one journal across seven starts. Two
mutations, each verified and restored byte-for-byte: skipping the re-revoke
of journal-held tools lets the pass after the deleted line be shadowed;
dropping the verbatim-repeat skip refuses the repeated policy as
reauthorising a tool that is not revoked.

## What this leaves open

The operator in a reauthorisation is the name the policy's author writes.
Nothing in `qip-cli` authenticates it. The platform's authenticated operator
(`qip_risk_engine::autonomy::OperatorIdentity`) is built only in `qip-api`'s
authentication middleware, and the CLI has no credential to check. Two ways
close it, and both are outside this ADR: a reauthorisation signed under an
ADR 0110 artefact class, which needs a fifth class and an HSM key that
`infrastructure/terraform/artefact-signing.tf` does not create; or an
authenticated `qip-api` route that writes the record, which needs the agency
journal to be one `qip-api` can reach. Until one exists, the journal shows
who was named and why; it does not prove who wrote the name.

A revocation is journaled only with a pass. A start that revokes a tool and
runs no pass leaves nothing on the journal, so a later deletion of that line
readmits a tool that never acted while revoked.

## What it costs

A tool once revoked on a journal cannot come back by editing one line. The
operator writes a reauthorisation with a name and a reason, and leaves it in
the policy or removes it afterwards; either is accepted. Two record kinds
join the journal, and every exhaustive match over `AgencyRecord` grows two
arms.

## What would make this wrong

- If the journal stops being the agency's durable record — a deployment that
  starts each run on a fresh journal resets every revocation, and this ADR
  then protects nothing.
- If an authenticated channel for the reauthorisation lands and the policy
  field stays accepted beside it, the weaker path becomes the one used. The
  field should be withdrawn when the authenticated one exists.
- If the guard's own revocation semantics change so `reauthorise` no longer
  refuses a blank operator, decision 3 loses its first check silently.
