# A figure a cell cannot evaluate

*Settled 2026-09-06. Overturning this needs the evidence named at the end, not
an opinion.*

## The finding

`qip-risk`'s `RiskState` carries `unevaluated: BTreeMap<String, String>` — the
figures a producer set out to compute and could not, each with the refusal that
stopped it — and `qip-risk-engine`'s `PreTradeChecker::check` rejects every
order while one entry stands. It was added because a control whose input is
missing *abstains*, and at the venue an abstention and a pass are the same
event: `Platform::liquidity_ladder` refused one book, `liquidatable_within`
stayed empty, `LimitKind::MinLiquidity` took its `None` arm, and a universe
that had just refused ten orders out of ten accepted ten out of ten with one
spread changed.

An independent measurement then found that this is the central path only:

```
$ grep -rn "RiskState\|PreTradeChecker" backend/crates/edge
$ echo $?
1
```

Nothing. Re-run it; if it ever returns a line, this document is out of date and
the reason it changed needs stating here.

## The question

Two readings fit that grep, and they have opposite consequences.

1. **The envelope argument.** A cell decides alone (ADR 0008) inside a signed,
   bounded, expiring `VerifiedEnvelope`, and reaches a venue through its own
   gates. The regional plane not importing the centre's risk state is the
   architecture working. Importing it would put a service's type in the hot
   path and give one fact two homes, which
   `.claude/rules/architecture/00-boundaries.md` forbids in as many words.
2. **The hole.** A cell sizes against a figure it could not evaluate, in
   exactly the way the centre no longer can, and nobody noticed because the two
   planes were reviewed separately.

## The answer: (1), and here is why

Reading `Cell::work` end to end, **every figure the cell sizes against is
either measured from the cell's own book on this pass or refused under a named
gate literal before an `Intent` exists.** There is no abstaining control on the
edge path, so there is nothing for an `unevaluated` map to hold.

The mechanism differs from the centre's and that is the point. `unevaluated`
exists because at the centre the *producer* of a figure (`liquidity_ladder`)
and the *consumer* of it (`MinLiquidity`) are different components that meet
through a struct, so the producer's failure has to be carried across the gap as
data or it is lost. In a cell the producer and the consumer are the same
statement. Every figure comes from the book the cell is holding or from
arithmetic on it, and every way of not getting one is a control-flow branch
with a refusal in it, not a `None` filed in a map for someone downstream to
notice.

The branches, as of this writing (`Cell::intent_for`, `Cell::admit_feasible`,
`Cell::admit_cycle`, all in `backend/crates/edge/qip-edge/src/cell.rs`):

| Figure | Cannot be evaluated when | Gate literal |
|---|---|---|
| A venue quoting the instrument | no book, or the only book is `Unreachable` | `venue_selection` |
| The book itself | a sequence gap abandoned it | `stale_book` |
| Tradability | the venue is `Halted` or `Closed` | `venue_status` |
| The reference price | the book serves no mid | `pricing` |
| The send price | the deployment stated no `PricingPolicy` | `pricing` |
| The narrowed size | `desired × multiplier` is not representable, or is zero | `degradation_sizing` |
| The held notional | `quantity × price` is not representable | `region_reservation` |
| Depth at the touch | the side has no resting level | `feasibility_depth` |
| The cycle's edge per unit | `net ÷ start_quantity` is not representable | `arbitrage_cycle` |
| A leg's fixed cost fraction | the leg has no positive notional | `feasibility_fee_floor` |

Two properties of that list matter more than its length.

**Absence narrows rather than passes.** With no policy payload at all,
`Cell::narrowing` returns `DegradationState::nothing_known()`, which reads every
payload-fed capability of §6.2 as `Unavailable` and produces a sizing multiplier
strictly below one. A cell whose policy feed has died sizes smaller, not the
same. That is the identical asymmetry `unevaluated` enforces, reached by the
table rather than by a map.

**Narrowing is not halting, deliberately.** ADR 0008 says a cell that cannot
reach the centre keeps working within its envelope. So the uninformed cell still
trades — at the floor. The centre's `unevaluated` refuses outright because the
centre has no envelope bounding it and no reason to keep going; the cell's
envelope *is* the bound, and it was approved by somebody before the partition.
These are different disciplines because the two planes are in different
positions, not because one of them was forgotten.

## What is deliberately not a control, and why that is stated

`feasibility::assess` runs its depth rule always, and runs the minimum-quantity,
lot, tick and minimum-notional rules **only where a `VenueModel` or the policy
payload's item 11 supplies the number**. A venue nobody modelled is checked for
depth alone. That is the same *shape* as `MinLiquidity` abstaining, and it is
still correct, for a reason the module comment gives: a lot size guessed at is a
rounding rule wearing a refusal's clothes. The difference from the liquidity
floor is that nothing here attempted the figure and failed — the operator never
stated it. There is no producer whose silence could be misread, so there is
nothing to file. `a_venue_with_no_model_and_no_constraints_is_checked_for_depth_alone`
pins it so the omission cannot be mistaken for an oversight later.

The same applies to `fixed_cost_fraction`, which contributes zero for a venue
with no fee source. Zero is what an unstated fee costs, and the `None` arms say
so rather than a default hiding it.

## How the claim is enforced

`backend/crates/edge/qip-edge/tests/unevaluated.rs`. A premise test proves the
intact fixture sends exactly one order and refuses nothing — without it every
row below would pass on a cell that never traded. Then a table withholds one
figure at a time and asserts, for each, that no order went out and that the
refusal was counted under the *named* gate. Naming the gate is load-bearing:
under mutation, disabling the stale-book branch still sent nothing, because the
pricing gate caught it a few lines later — defence in depth, and also exactly
how a control silently stops firing while the system still looks safe. A test
that asserted only "no order" would have passed that mutation.

The third test pins the floor: with no payload, `causal_graph`,
`episodic_memory` and `belief_state` all read `Unavailable`, the multiplier is
strictly between zero and one, and the order that goes out is the ask times that
multiplier.

**Duty on the next author.** No test can discover a control that was never
written. If you add a gate to `Cell::work` that reads a figure, add its row to
that table in the same change.

## What would change this decision

Any one of these, and (2) becomes the right reading:

- A figure reaching `Cell::work`'s sizing arithmetic from **outside** the cell —
  a centre-computed liquidity or volatility number arriving on the policy
  payload and consumed as a multiplier or a bound. That reintroduces the
  producer/consumer gap `unevaluated` was invented for, and the cell would then
  need its own way to carry "the centre tried and failed" across it, in the
  edge's idiom: a journaled refusal under its own gate, not an imported map.
- A gate in `Cell::work` that reads a figure and takes a `None` arm without
  refusing — the abstention shape, wherever it appears.
- Evidence that a refusal counted under one gate is being read as another, so
  that the operator's chart says a control fired when a different one did.

None of these is hypothetical-only: the first is one policy-payload slot away.
Which is why the grep above being empty is not, on its own, the argument.
