# An agent attempted an ungranted capability

## Do this

1. **Nothing urgent.** The attempt was refused. The control worked.
2. Find which agent and which capability:
   ```sh
   curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/system/governance
   ```
   The run record carries the denied access with the capability, the facility
   and the timestamp.
3. Decide which of two things happened.

## It is a bug

The agent's code reaches for something its manifest does not grant. Either the
code is wrong or the manifest is.

If the manifest should grant it: change the manifest, and check that
`AgentManifest::validate` still accepts the result. Several combinations are
refused outright — a research-side agent holding anything market-touching, a
proposer holding a veto, an execution agent that can publish theses — and the
refusal is the point.

If the code is wrong: fix the code. The gate did its job.

## It is an agent doing something nobody anticipated

Rarer and more interesting. An agent taking a code path that reaches for a
facility under conditions nobody thought about is worth understanding before it
recurs, even though it was contained.

## Why this is recorded rather than merely blocked

`AgentContext::authorise` writes the audit entry *before* returning the error,
so an agent probing for capabilities it does not have leaves evidence.
`AuditTrail::permission_violations` surfaces every run where one happened.

A control that silently refuses tells you the system is safe. A control that
refuses and records tells you what tried.
