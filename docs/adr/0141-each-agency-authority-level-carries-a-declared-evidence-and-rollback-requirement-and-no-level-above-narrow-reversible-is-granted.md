# ADR 0141: Each agency authority level carries a declared evidence and rollback requirement, and no level above narrow-reversible is granted

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It writes down a schedule and grants
  nothing beyond what the code grants today; ADR 0114 D8's refusal of
  external action and ADR 0003/0021/0108's refusal of live capital are
  untouched.
- **Date**: 2026-10-18
- **Extends**: `qip-agency/src/autonomy.rs`'s module documentation ("There is
  deliberately no rung above narrow-reversible ... what it would be is a
  decision for an ADR").
- **Keeps**: ADR 0114 D8, ADR 0004 (agents reach facilities only through
  capability gates), the separation from the trading autonomy ladder
  (`qip-risk-engine`'s `AutonomyController`), which nothing here can reach.

## Context

`qip_agency::autonomy::Authority` has two rungs: `Shadow` (plans, simulates,
gates, reports; no adapter is called) and `NarrowReversible` (may call an
adapter for a step of its own class whose tool is declared reversible).
`ActionPolicy::widen` moves Shadow to NarrowReversible only on an identified
`EffectAttribution` for the policy's own class *and* an approver holding
`agency:widen-autonomy`, and refuses anything else with "no authority above
narrow-reversible exists; broader agency needs an ADR"; `observe` drops a
policy back to Shadow on drift with nobody asked. R12-EXPAND-6071 ("evidence
and rollback requirements scale with authority") is PARTIAL because with one
step there is no scale to state.

Locate them with
`grep -n 'pub enum Authority\|pub fn widen\|pub fn observe\|WIDEN_AUTHORITY' backend/crates/services/qip-agency/src/autonomy.rs`.

## Decision

1. **The schedule is a typed table, and `widen` reads it.** Each level names
   its scope, the evidence that admits it, the rollback it must carry, the
   number of distinct approvers, and whether any record grants it. `widen`
   checks the table rather than a hard-coded branch, so a later record that
   grants a level changes one cell, never the evidence it demands.
2. **The levels and what each requires:**

   | Level | Scope | Evidence | Rollback | Approvers | Granted |
   |---|---|---|---|---|---|
   | Shadow | none | — | — | — | default |
   | NarrowReversible | own class, reversible internal tool | one identified attribution for the class | the tool's declared reverse | 1 | yes (today's rule) |
   | NarrowCompensable | own class, internal tool whose effect is undone by a separate compensating step | identified attribution plus a stated period at NarrowReversible with no drift | a compensating step, tested in a drill and named on the policy | 2 | **no** |
   | BroadInternal | several classes, internal tools | identified attribution per class, each at NarrowCompensable without drift | compensation per class, and a kill path that returns every class to Shadow at once | 2 | **no** |
   | External | any outward surface (posting, outreach, pricing, operations) | — | — | — | **refused by ADR 0114 D8** |
   | Capital | any capital, hedging or liquidity surface | — | — | — | **refused by ADR 0003, 0021, 0108** |

3. **No level above NarrowReversible is granted by this record.** `widen`
   refuses NarrowCompensable and BroadInternal with a message naming this
   record and saying a further record must grant the level; it refuses
   External and Capital naming the record that refuses them. The difference
   is deliberate: the first two are ungranted, the last two are prohibited.
4. **Evidence never substitutes for an approver, and an approver never
   substitutes for evidence**, at every level, as `widen` already holds for
   the first step. Drift drops a policy to Shadow from any level.

## Consequences

- The row's property (evidence and rollback scale with authority) is stated
  as data a test can read, and enforced for the one step that exists.
- Code slices left: the table in `autonomy.rs`, `widen` reading it, the two
  new ungranted variants, and `tests/agency.rs` asserting each level refuses
  the lesser evidence, the ungranted levels refuse with this record's name,
  and External and Capital refuse with theirs (EXPAND-6071). Each needs its
  mutation.

## What it costs

- Two enum variants nothing can reach; a reader may mistake a declared level
  for an available one, which is why the refusal messages differ.
- The numbers in the table (the period without drift, the approver counts)
  are chosen, not measured, and will be argued over when a record proposes
  to grant a level.

## What would make this wrong

- If a granted level were ever reached by a path other than `widen` (a
  policy deserialised at a higher level, say), the table would be bypassed;
  decode must route through the same check.
- If internal actions turned out not to be separable into reversible and
  compensable (an internal tool whose compensation is itself irreversible),
  the NarrowCompensable row is wrong, and the tool should stay at Shadow.

## Rows

R12-EXPAND-6071. It stays PARTIAL until its slices land with a mutated test.
