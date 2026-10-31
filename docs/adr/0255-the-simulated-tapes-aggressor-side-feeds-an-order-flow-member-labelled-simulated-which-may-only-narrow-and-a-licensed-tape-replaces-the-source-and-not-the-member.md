# ADR 0255: The simulated tape's aggressor side feeds an order-flow member labelled simulated, which may only narrow; a licensed tape replaces the source and not the member

- **Status**: **Accepted 2026-10-31 under the owner's delegation**. This is
  decision 10 of `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`,
  relayed by the integration parent, which assigned R12-WORLD-3046 to lane
  L2-INTEL. No risk number or threshold is set, no limit changes, and no
  paper-trading layer is touched.
- **Date**: 2026-10-31
- **Amends**: ADR 0151 decision 6 ("Signed order flow has no source, so no
  flow member is built"). That ADR is left as written. This record states the
  amendment.
- **Rows**: R12-WORLD-3046.

## Context

ADR 0151 decision 6 declined an order-flow member because no source stated a
trade's aggressor side. It named a licensed tape with aggressor side as the
source to wait for. Since then the paper feed has stated each trade's
aggressor side: `qip_market::quote::Trade::aggressor`, filled by the depth
dataset adapter. The simulated depth dataset's terms are the platform's own,
and that dataset already feeds the tape members. The side reached the market
snapshot and nothing modelled it.

## Decision

1. **The source is the paper feed's aggressor side, labelled simulated.**
   `Platform::record_order_flow` adds each trade's size to that cycle's
   signed flow per instrument and venue. It reads only `Trade::aggressor`.
   A trade whose venue stated no side contributes nothing. Inferring a side
   from the quote would be the platform inventing the flow it then forecasts
   from. Neither does a print that formed no price: a late report, a
   correction or an off-exchange cross. A block crossed away from the book
   says who traded, not who was taking the spread, and
   `qip_market::microstructure` counts only price-forming prints too.
   UNDERSTAND takes the cycle's flow, so the member reads one cycle's
   prints and never an accumulating history. The member declares its source
   as `sense:trade-aggressor:simulated` (`ORDER_FLOW_SOURCE`), so every
   listing of the population says where its evidence comes from.
2. **The member is `markets.order-flow.global.micro`.** It feeds each
   instrument's existing next-close proposition, the one the tape members
   answer, with `logistic(ORDER_FLOW_SLOPE × imbalance)`. The imbalance is
   `(buy − sell) / (buy + sell)` summed across venues. Evidence fit grows
   with the signed prints, up to twenty. Where no print stated a side, the
   member is silent rather than even, because a tape that named no aggressor
   told it nothing. It is resolved by that proposition's next close, like
   every tape member.
3. **It may only narrow.** The federation's one power over DECIDE is ADR
   0125 decision 4's narrowing. A material spread among members maps to a
   multiplier below one, with no arm above one. A new member can widen that
   spread and so narrow a bound. It cannot enlarge one.
4. **A licensed tape replaces the source, not the member.** When a licensed
   tape with aggressor side is admitted under ADR 0157, its trades enter
   through the same `Trade::aggressor`, and the declared source changes. The
   member, its proposition and its resolver do not.

## Consequences

- `qip-kernel/tests/symbolic_federation.rs::signed_trade_flow_gives_the_order_flow_member_a_view`
  feeds AAA sell-dominated prints. It feeds BBB prints that name no side,
  plus a large off-exchange cross that does name one. The member gives AAA
  one view below 0.5 and gives BBB none. The next cycle has no new prints
  and finds the member silent. Three mutations each fail the test: reading
  every print as a buy, dropping the price-forming filter, and reading the
  flow by `clone` instead of `take`. That the member can only narrow follows
  from the federation's narrowing (decision 3). This test does not exercise
  it.
- The population grows by one member. The counts in the federation tests
  move with it, and on a tape with no signed prints the member is listed
  among the silent ones.

## What it costs

- The member's evidence is the platform's own simulator. Its record measures
  how well simulated aggressor flow forecasts simulated closes, and that says
  nothing about a real market. The source label is what keeps a reader from
  taking it as more.
- One cycle's flow is a thin signal, and the slope is declared, not fitted.
  The member will often add disagreement that narrows a bound which needed no
  narrowing. That error costs paper size, not capital.
- Per-cycle flow per instrument and venue is held until UNDERSTAND takes it.
  It is bounded by what one cycle's trades name.

## What would make this wrong

A feed that fills `Trade::aggressor` by inference rather than from the
venue's own statement. The member would then read the platform's guess as
evidence, which decision 1 refuses. The guard belongs at that adapter.
