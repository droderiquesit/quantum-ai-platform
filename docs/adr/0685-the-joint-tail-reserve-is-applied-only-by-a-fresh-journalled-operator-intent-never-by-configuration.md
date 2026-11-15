# ADR 0685: The joint tail reserve is applied only by a fresh, journalled operator intent, never by configuration

- Status: Proposed (lane 42, wave b52; built, not compiled)
- Date: 2026-11-15

## Context

R12-RISK-4116: every SIMULATE computes a generated joint tail world but reports
it as advisory. Applying it raises the survival reserve, which the owner has not
confirmed in their own words (owner-decisions-2026-10-24 excludes risk-limit
changes from a general delegation). An earlier draft called
`Platform::set_joint_tail_reserve_applied` from `qip-api`'s `main`, which
`api_boundary` forbids: the API raises an intent and never mutates the platform.

## Decision

1. `Platform::authorise_joint_tail_reserve(&OperatorIdentity, now)` is the typed
   intent. It refuses a credential older than `JOINT_TAIL_CREDENTIAL_AGE`,
   journals a `JointTailReserveAuthorisation` (operator subject) before the
   switch moves, then applies the world from the next SIMULATE.
2. It only tightens: no counterpart switches it off. The default stays advisory
   until an operator acts, so shipping this changes no reserve.
3. `JOINT_TAIL_CREDENTIAL_AGE` is **provisional**: the existing 15-minute
   eligibility age is reused; no new figure was invented.
4. The HTTP route that would raise the intent needs a new row in
   `api_boundary.rs`'s reviewed-route table, a protected guard suite. That is
   NEEDS_PERSON; no route is added.

## What it costs

Until the API route exists and the owner confirms, the reserve stays advisory,
so the generated joint tail world protects nothing: an operator can reach the
intent only through code that holds a `Platform`, and no HTTP row for it is
added. Once an operator does act, the intent only tightens and has no
counterpart, so each later SIMULATE on that `Platform` that generates a joint
world applies it to the survival reserve, and the intent offers no way to stop
that; the capital that reserve withholds is unmeasured. The switch is a field
that starts off in a newly built `Platform`, and
`replay_joint_tail_reserve_authorised` only reports whether the log holds an
authorisation; nothing sets the switch from it. The old `Platform::set_joint_tail_reserve_applied`
setter is left in place, so a caller holding a `Platform` can still flip the
switch without a credential or a journal record; closing that is not done here.
The 15-minute credential age is provisional and unmeasured against how long an
operator takes to decide. Remaining: the API route, and the owner's
confirmation that raising the reserve is accepted. Paper boundary untouched
(Terraform refusal, `AutonomyLevel::deployable`, `Cell` constructor unchanged).
Nothing compiled or tested.

## What would make this wrong

If the owner decides the reserve should be reversible, the one-way switch is
wrong and needs a journalled counterpart. If a composition root ever calls the
unauthenticated setter (none does; a test in `qip-kernel/tests/stress.rs`
does), the guarantee that only a fresh journalled intent applies the reserve
does not hold and the setter must be removed or restricted. If the applied reserve withholds enough capital that the
desk cannot trade, or operators routinely cannot authorise inside the
credential age, applying the world or the age should be revisited. If
`replay_joint_tail_reserve_authorised` disagrees with the running switch, the
switch and the log have parted and the log alone no longer says what is
applied; the setter already allows that.

## Addendum: lane 42's other rows (this lane owns only two ADR numbers)

- **R12-GOV-6127.** Decision: nothing in the requote seam moves.
  `qip-edge-node/src/reprice.rs::replace` and its gate stay where
  `order_route_risk_gate` finds them. A per-cycle non-profit ground for an
  arbitrage leg does not exist in the tree, and whether a mandated arbitrage is
  outside what the requirement forbids is the owner's decision (ADR 0200, Known
  limits 2). Until then the restrictive state holds: the standing grant anchors
  a leg and the skeptic's bar 4 stays open. NEEDS_PERSON. Nothing built.
- **R12-ASSET-4070.** Decision: only gap 4 (net versus gross) is built, because
  it can only refuse more. Kernel wiring of group caps stays withdrawn (f1ff11cf:
  a non-binding row changed the solver's feasibility report on the fast-brain
  tape); it cannot be validated without running the tape, which this lane did
  not do. Strategy, region and horizon keys on `ApprovedThesis` are not added.
- **R12-CAPITAL-1161.** See the report; arming needs a declaration and figures
  nobody gave.
