# 0004 — Agents reach facilities only through capability gates

**Status:** accepted

## Decision

Every facility on the shared desk — market data, the world model, the book, the
risk state, the compliance register, research memory, the document library — is
wrapped in a `Gated<T>`. The inner value is private, and the only accessor
takes the agent's run context.

Reaching a facility therefore necessarily: checks the manifest's capability
grant, charges the run's budget, and writes an audit entry.

## Why

The alternative is asking each agent to check its own permissions. That works
until an agent is written by someone who does not know the convention, or until
an existing agent grows a new code path that skips the check.

Containment that depends on the contained component cooperating is not
containment. The property worth having is that an agent which *forgets* to
check is still contained, and `Gated<T>` gives exactly that.

The audit entry matters as much as the check. A denied access is recorded
before the error is returned, so an agent probing for capabilities it does not
have leaves evidence — and `AuditTrail::permission_violations` surfaces it.

## What it costs

Every facility access is a method call with a context argument, which is more
verbose than a field access. Eighteen agents pay that cost.

It also means a facility cannot be borrowed across a long computation without
holding the context, which occasionally forces a clone.

## What would make this wrong

If the set of facilities grew large enough that the manual pairing in
`Desk::new` became unwieldy, a derive macro would be better. The property would
be unchanged; only the boilerplate would move.
