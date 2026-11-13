# ADR 0472: The desk refuses an order that would trade against a working order of its own on the other side of the same instrument at the same venue

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-23 (wave580). The delegation's limits hold: it sets no figure,
  changes no requirement's wording, reverses no accepted refusal, adds no
  dependency and touches no paper-trading layer. It only adds a refusal.
- **Date**: 2026-11-12
- **Rows**: R12-GOV-6126 (stays PARTIAL; this closes one of its four gaps).

## Context

R12-GOV-6126 asks that market-facing actions be cross-checked for
manipulation risk, verified by "a market-facing action matching a
manipulation pattern is refused before any adapter". The register's gap
(3) named the desk's missing wash/self-trade refusal: the edge cell refuses a
net that would cross a resting order of its own under the `self_trade` gate
(`qip-edge` `Cell::would_self_trade`), and the desk's order manager —
`qip_execution_engine::oms::OrderManager::submit`, the one seam a desk order
reaches a broker through — refused nothing of the kind. A desk sale of an
instrument at a venue where a buy of the desk's own rested went to the
venue, which would match the two: a trade with no change of beneficial
ownership, printing volume nobody traded.

## Decision

1. `OrderManager::submit` gains step 6c, after the place-and-cancel conduct
   check (6b) and before pre-trade risk and the broker: an order is refused
   when an accepted order of the desk's own is still open at the same venue
   (the broker's name, as every venue fact in the manager is keyed), on the
   same instrument, on the other side, with quantity left to fill, **of a
   type that rests in a book** (every type but `OrderType::Market`), **on a
   side whose remainder the venue keeps working** (`Broker::works_remainder`).

   A market order's remainder takes liquidity and rests nowhere, and two
   aggressive orders of the desk's own each take resting liquidity rather
   than each other. A wash trade needs a resting order of the desk's own
   on one side. The scheduled and participation types are counted, because
   their child orders' prices are not visible here. Counting them costs a
   refusal; missing them would let a wash trade through.
   Only such a remainder rests where a later order could match it. The port
   answers `true` by default, which fails closed for a real or unknown
   venue. `SimulatedBroker` answers `false` for a purchase, because ADR 0424
   has it work a sale's remainder and never a purchase's.

   The first cut of this record had no such condition. With the integration
   branch merged, it refused twelve `qip-kernel` integration tests. Each
   test bought, was left with an open purchase remainder no venue worked,
   and was then refused its sale as a wash trade, so the desk could never
   sell a partly filled purchase. The lane had run only `qip-kernel --lib`,
   and the orchestrator caught it before the merge. The works-remainder
   condition fixed eleven of them. The twelfth, in `margin_funding_limits`,
   sold out of a margin call with a partly filled market sale and was then
   refused its buy. That case is what the market-order condition answers:
   the desk path sends market orders, and their remainders never rest.
2. **Any** opposite working order counts, whatever either limit price is.
   Whether two limits cross depends on the venue's matching rules; a price
   test here would be a model of them nobody checked, and a wrong one lets a
   wash trade through. The cell refuses on the same broad terms. The cost is
   that a desk that wants to quote both sides of one book at one venue
   cannot; the desk has no market-making path, and a later one would need
   its own record.
3. Refused, not netted and not withdrawn: the resting order is the desk's
   earlier decision, with a time to live or a cancel somebody chooses.
4. The refusal is a new variant, `RefusalReason::SelfTrade { venue,
   resting_order, detail }`, additive on the wire as `Conduct` was. It is a
   posture of the desk's own book, so it is neither a safety control nor
   sizing evidence, and it is charged to no limit-set rule. The kernel's
   `gate_of` charts it as `conduct-self-trade`, beside `conduct-spoofing`,
   so one fixed literal is added to the label and no runtime string is.
5. The check walks an index of accepted-and-open orders
   (`working_venues`, pruned exactly as `working_axes` is), never every order
   ever sent, so a long-running desk pays nothing more per submission.

## What it costs

- The desk's cycle sends market orders, so on its default path the check
  fires only against the limit orders the quote loop rests. It fires on the
  orders a venue could actually match, and that is the trade it accepts.

- The rule trusts each venue's own answer about which remainders it works.
  A simulated venue that answered `false` wrongly would let a desk order
  through against a remainder that does rest; the port's `true` default and
  the ADR 0424 test of the simulator are what guard that.

- The desk cannot rest orders on both sides of one book at one venue. A
  two-sided quote would be refused on its second leg. The desk has no
  market-making path today, and a later one needs its own record and a
  price-aware rule.
- A desk that wants to reverse while a resting order is still open must
  cancel that order first. Before this change it could send the opposite
  order straight away; now that takes one more step.
- One more label value, `conduct-self-trade`, on the kernel's refusal
  series, and one more `working_venues` entry per open order, pruned at
  every check.

## What would make this wrong

- The desk comes to need two-sided quoting at one venue. Then the broad rule
  refuses legitimate quotes, and a price-aware rule against the venue's
  documented matching becomes worth its risk.
- A venue offers self-trade prevention that the platform can verify per
  order. The venue would then refuse the cross itself, and the desk's
  refusal would duplicate it. It would still be kept until that prevention
  is shown on the venue's own record.
- Desk orders begin working at a venue the broker's name does not identify.
  The venue key is then wrong, and opposite orders at one venue can pass.

## Not decided here

Marking the close, momentum ignition and quote stuffing (gap 3's remainder);
a production caller of `OrderManager::cancel` (gap 1); a production path for
the communications check (gap 4). R12-GOV-6126 stays PARTIAL on those.

## Evidence

`cargo test -p qip-execution-engine --test execution
a_desk_order_that_would_trade_against_its_own_working_order_is_refused_before_the_broker`,
mutation-verified twice: disabling step 6c fails it at "a sale crossed the
desk's own resting buy"; dropping the opposite-side condition fails it at "a
second buy was refused". `a_purchase_remainder_the_simulated_venue_never_works_does_not_refuse_a_sale_but_a_sale_remainder_refuses_a_buy`
in the same suite, mutation-verified twice: dropping the works-remainder
condition fails it at "a sale was refused against a purchase remainder
nobody works"; making the simulator answer `false` for a sale fails it at
"expected a self-trade refusal against the sale". Counting a market order's
remainder fails the first test at "a sale was refused against a market
order's remainder".
