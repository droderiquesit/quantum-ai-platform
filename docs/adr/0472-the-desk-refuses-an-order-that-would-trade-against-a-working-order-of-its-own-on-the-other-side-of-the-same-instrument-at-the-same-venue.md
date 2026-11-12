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
   same instrument, on the other side, with quantity left to fill.
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

## Not decided here

Marking the close, momentum ignition and quote stuffing (gap 3's remainder);
a production caller of `OrderManager::cancel` (gap 1); a production path for
the communications check (gap 4). R12-GOV-6126 stays PARTIAL on those.

## Evidence

`cargo test -p qip-execution-engine --test execution
a_desk_order_that_would_trade_against_its_own_working_order_is_refused_before_the_broker`,
mutation-verified twice: disabling step 6c fails it at "a sale crossed the
desk's own resting buy"; dropping the opposite-side condition fails it at "a
second buy was refused".
