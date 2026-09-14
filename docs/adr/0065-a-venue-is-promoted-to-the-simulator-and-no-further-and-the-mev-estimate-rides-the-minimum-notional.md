# ADR 0065: A venue is promoted to the simulator and no further, and the MEV estimate rides the minimum notional

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default), ADR 0002 and ADR 0009 (two dependencies), ADR 0062 (a venue is withdrawn on feasibility evidence and reinstated only by two signatures)

## Context

Blueprint §34.3 and §34.4 were both `ABSENT`, and they are the same subject
seen from two ends: what the platform must know about a venue before it uses
it, and what it must have measured before it is allowed to.

§34.3 says a decentralised exchange is not an order book behind a different
protocol, and names four pieces a DeFi adapter must carry — pool-math
slippage, a block-time execution mode, an MEV and front-running estimate that
the feasibility gate reads, and a contract-risk flag the risk envelope treats
as a distinct exposure — with observe-only registration as the fallback where
any is absent.

§34.4 draws a six-rung ladder — registered, observed, simulated, shadow,
capped live, full — with a gate per rung, and closes on the sentence the whole
section is for: "a venue is never enabled from its own documentation alone".

Four facts about this workspace shape what could honestly be built.

1. **There is no live-class venue adapter.** `qip_brokers::AdapterClass` has
   two variants, `Simulated` and `Sandbox`, no third, and no string that
   deserialises into one. Three of §34.4's six rungs — shadow, capped live,
   full — describe a connection this build cannot make.
2. **The feasibility gate vocabulary is a bounded label set, not a free
   string.** `qip_contracts::feasibility::EDGE_GATES` is the set
   `qip_feasibility_refusals_total{constraint}` is keyed on and the set the
   centre admits a carried refusal by; a gate literal invented in a service
   crate arrives at the centre as `other`, which is the label meaning "a plane
   used a name this build does not know".
3. **`GateStage` is already a promotion ladder**, with one-step-at-a-time
   promotion, evidence-bearing gates, authority-free demotion, and
   `holds_capital()` as a property of a rung rather than of a caller.
4. **A minimum notional is the rule that already asks "is this order worth
   doing here"**, and on a chain the answer turns on a fixed per-transaction
   cost — gas — against a proportional cost the trade pays for crossing.

## Decision

**Three decisions, each of which could have gone the other way.**

### 1. The venue ladder is `GateStage`, and its ceiling is `Paper`

§34.4's rungs are mapped onto `qip_contracts::gate::GateStage` rather than
onto a new enum: registered → `Candidate`, observed → `Holdout`, simulated →
`Paper`. `qip_lifecycle::venue_ladder::VENUE_PROMOTION_CEILING` is
`GateStage::Paper`, `attempt_promotion` *computes* its target from
`GateStage::next` and refuses anything above the ceiling, and there is no
parameter naming a target and no policy field that raises it.

`Holdout` for "observed" is the exact correspondence rather than the obvious
one: a holdout rung judges a claim against data held out of the fitting that
produced it, and §34.4's observed rung judges a venue's declared fees, latency
and order types against measurement taken independently of those
declarations. Both refuse a number checking itself.

**Promotion in this platform means eligible for the simulator.** It does not
and cannot mean eligible for real execution. `VenueLadder::admits` is the one
question the type answers about use, and there is no `admits_capital` on it.
`OrderManager::submit`'s `LiveVenueBelowLiveAutonomy` step is untouched and
does not consult the ladder, so the ladder is not a fourth way past the three
paper-trading layers; it is a control that only ever subtracts reachability
from a venue that has not earned it.

### 2. The MEV estimate is carried into the minimum notional, not into a new gate

`qip_brokers::dex::DexVenue::feasibility_model` derives a
`qip_execution_engine::feasibility::VenueFeasibility` whose minimum notional
is

```text
    gas × 10000 / (budget_bps − slippage_bps − extractable_bps)
```

so that a venue offering more headroom to the mempool demands a larger ticket
before an order there is worth submitting. The derivation is exact rather than
rhetorical: gas is charged per transaction whatever the size and is therefore
the only term that produces a *minimum* at all, while slippage and the
adversary's headroom are proportional and so consume the budget gas has to fit
inside.

Refusals land under `feasibility_minimum_notional`, a literal both planes and
the centre already know, and the label set stays bounded.

Where slippage and headroom exhaust the budget on their own,
`feasibility_model` **refuses** rather than returning a minimum notional no
order would reach. A threshold larger than the book is a refusal wearing a
threshold's clothes: an operator reading it is told the order was too small
when the truth is the venue is too expensive at any size.

### 3. Contract risk is its own exposure axis

`qip_financial::pool::CONTRACT_RISK_AXIS` is `"contract"`, deliberately not
`qip_risk::limits::COUNTERPARTY_AXIS`'s `"counterparty"`. A counterparty limit
is about an entity that can be called, sued, or asked to explain a break; a
contract limit is about code that can do none of those. Netting the two would
let a book held entirely through one unaudited contract read as diversified
across many dealers.

## What it costs

**A ladder with three of six rungs missing is a ladder somebody will want to
extend**, and the extension is the dangerous one. `rung_name` names shadow,
capped live and full so a refusal can be specific, which means the three rungs
are *visible* in the source. Anyone raising `VENUE_PROMOTION_CEILING` gets a
build that compiles, because the promotion machinery is general; what stops
them is `attempt_promotion`'s trailing arm, which refuses a rung no gate
admits to, and the tests that assert the ceiling holds no capital. That is a
smaller barrier than "the code does not exist" and it is the price of reusing
one ladder rather than writing a second.

**The minimum-notional derivation hides its own reasoning from the refusal
message.** `VenueFeasibility` carries four numbers and no rationale, so an
order refused under `feasibility_minimum_notional` at a decentralised venue is
told it is below a threshold, not that the threshold is what it is because the
venue's slippage tolerance leaves a hundred and fifty basis points to the
mempool. The derivation is documented at the constructor and tested, and an
operator reading only the refusal will not see it. The alternative — a gate
literal that says `mev` — costs an unbounded label at the centre, which is
worse.

**The MEV model is one bound and not a survey.** Headroom is a real bound and
it is not the only mechanism: a trade can be front-run without being
sandwiched, back-run, or censored, and none of those is priced here. A desk
that reads `extractable_bps` as "the MEV at this venue" will under-price the
others. The figure is named `extractable_bps` rather than `mev_bps` for that
reason, and the type's documentation says what it does not model in its second
paragraph.

**A ladder nothing is wired to is maintenance with no user.** Until `Platform`
holds a `VenueLadder`, the gates run only in tests. That is a real cost and it
is why the wiring is named precisely rather than left as "future work": one
field and one line, in a file this lane could not edit.

**Two crates gained an internal edge.** `qip-lifecycle` now depends on
`qip-financial`, and `qip-brokers` reaches `qip_execution_engine::feasibility`
to build a model it does not install. Neither is a third-party dependency and
neither inverts the layering, but both widen what a change to `qip-financial`
can break.

## What would make this wrong

**A live venue adapter.** The whole shape of decision 1 rests on there being
no live class to promote to. If one is ever built — which would need its own
ADR and would reopen ADR 0003 — then refusing the shadow rung stops being an
honest statement of what this build can do and becomes a ladder that refuses a
rung the platform has. The refusal message names ADR 0003 so that whoever
reopens it finds this line.

**A gas cost that is not fixed per transaction.** The minimum-notional
derivation is only correct because gas is charged per transaction whatever the
size. On a chain that prices execution proportionally, the derivation produces
a minimum where there is none, and the right answer would be a cost check
rather than a size floor.

**Evidence that a sandwich routinely takes less than the headroom.** The bound
is deliberately the worst case an adversary can reach. If measurement showed
extraction clustering well below it — because searchers compete the margin
away, or because the venue has private order flow — then sizing a promotion
bar off the bound would refuse venues that are fine, and the bar should move to
the measured distribution rather than the bound.

**A second writer of the promotion ladder.** `attempt_promotion` and `demote`
are the only two functions that write `VenueLadder::stages`, and `admits` is
equality with a constant. A path that set a stage directly — a deserialiser, a
`with_stages` constructor, a configuration loader — would make the ceiling a
convention rather than a structure, and it is the change to refuse in review.

## Consequences

**What the platform now refuses.** A decentralised venue missing any of the
four §34.3 pieces prices nothing and produces no feasibility model — it is
observe-only, which is the blueprint's own fallback rather than a rule
invented here. A venue whose measured fee or latency exceeds its declaration
beyond tolerance, whose declared order types were rejected or never tried, or
whose replay left a reconciliation break, does not leave the rung it is on. A
concentrated-liquidity pool refuses a trade that would move the price out of
its declared range rather than extrapolating into ticks nobody gave it. A
block-time execution mode with zero confirmations is refused, because a trade
nobody waited to confirm can be reorganised out of history after the platform
has booked it. And a promotion above the simulator rung is refused with an
approver present, on evidence that passed every gate.

**What it still cannot do, stated plainly.**

* **The four §34.3 pieces are a model, not a connection.** There is no chain
  client, no signer and no mempool subscription in this workspace, and
  `DexVenue` deliberately does not implement `VenueAdapter`: an adapter whose
  `submit_order` targeted a chain it cannot reach would be a stub that reads
  as a connection.
* **The MEV estimate is a bound, not a forecast.** It prices the headroom a
  trade leaves between the price the pool would give it alone and the worst
  price it signs for. It does not model gas auctions, priority fees or private
  order flow, and it does not claim an adversary is present.
* **The kernel-side review reports and does not withdraw.**
  `qip_kernel::venue_admission::review` names reachable venues that have not
  earned the simulator rung and returns strings. Withdrawing every unadmitted
  venue would, on the first deployment carrying an empty ladder, withdraw a
  deployment's entire venue list — a control firing correctly and stopping the
  platform, which is the safe direction but not a change anybody chose.
  `qip_kernel::venue_review` remains the only path by which a venue is
  withdrawn, on evidence of refusals rather than on an absence of records.
* **Nothing calls the kernel review yet.** `Platform` holds no `VenueLadder`.
  Until it does, §34.4 is delivered as a ladder with gates and a composition
  waiting for a caller, and the row should not be read as reached at the
  kernel.

**The paper-trading boundary is intact and unchanged.** Terraform still
refuses the three live ceilings at plan time; `AutonomyLevel::deployable`
still stops the process at each composition root; `qip-edge`'s `Cell` still
has no constructor taking a non-paper ceiling and `qip-cost-router`'s
`Determinism::Required` still returns a type that cannot name a model rung.
This ADR adds a fourth thing that cannot reach a live venue, and weakens none
of the three that already could not.

**Rejected alternatives.**

* *A `VenueStage` enum of six rungs.* Two answers to "what does it take to
  move something up a rung", drifting at the first change to either — and
  three of the six would name rungs this platform cannot reach, which is a
  ladder advertising a destination.
* *A `feasibility_mev` gate literal.* It would arrive at the centre as
  `other`, firing a drift alarm on a gate the build declares, and would need
  an edit to `qip_contracts` and to both planes' feasibility modules to avoid
  that.
* *Withdrawing unadmitted venues from the kernel.* See above: correct in
  principle, and on today's wiring it would stop the platform on the cycle it
  was introduced.
