# ADR 0399: A currency conversion on the paper desk is an FX spot fill, a financing valuation is reported and not marked, and a cross is priced only through its one route

- **Status**: **Accepted 2026-11-12 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session cov-a-01
  writes it. The limits of that delegation hold:
  - it sets no figure, limit or threshold;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency and no crate edge.
- **Date**: 2026-11-12
- **Extends**: ADR 0387 (one source for what is pledged), whose shape
  decision 1 follows for conversions. It amends no clause of any accepted
  ADR.
- **Rows**: R12-COVERAGE-5098.

## Context

R12-COVERAGE-5098 ("fiat, spot/forward FX, cash equivalents, funding and
currency conversion are supported") stopped on three gaps.

1. `qip_capital::bank::CurrencyBook::convert`, which debits one currency,
   credits the other and journals the rate, has no kernel or app caller.
2. An `FxSwap` was not valued, because `FxDetails` held one date.
3. A forward's outright was reported at UNDERSTAND and acted on by nothing:
   no position was marked to it and no size read it, and a currency with no
   pair against the base was left unpriced rather than triangulated.

Gap 2 and the cross in gap 3 are code and are closed beside this ADR
(`FxDetails::near_settlement_date`, `FinancingRegister::value_swap`,
`FinancingRegister::fx_rates`). The rest are design questions in the
repository, which decision 28 says a session decides.

## Decision

1. **A conversion is an FX spot fill, and the portfolio's record of it is
   the one record.** On the paper desk a currency is converted by an
   `FxSpot` order through the full desk control path, filled on the
   simulated broker and stamped simulated, as
   `qip-kernel/tests/asset_classes.rs::fx_cash_and_digital_asset_instruments_assemble_and_trade_on_paper`
   proves. The platform books that fill as a position in the pair, as it
   books every fill. A `CurrencyBook` opened in the kernel beside it would be
   a second claim about the same cash, fed by whoever opened it, and the two
   would disagree the first time one of them refused what the other booked —
   the book refuses to sell a balance it does not hold; the portfolio holds a
   short in the pair. Decision 28's fourth preference keeps the one record.
   `CurrencyBook` stays a tested model of its rule (both legs together or
   not at all, refused rather than cut to fit) in
   `qip-capital/tests/bank.rs`, and nothing that ships constructs one.
   `qip-acceptance`'s `currency_book_source` suite refuses a production
   source outside `bank.rs` that names it, as `pledge_source` does for
   `PostingBook` under ADR 0387.
2. **A forward or swap outright is reported and not marked.** The platform
   marks no instrument of any class: equity is realised-only and positions
   are carried at cost, a stated exclusion in `Platform::risk_state` and
   `Platform::equity`. Marking FX forwards alone would put two conventions
   into one equity figure, a class at its outright beside every other at
   cost, and the risk monitor would read a number that is neither. The
   consumer the gap names, a mark-to-market plane, does not exist in the
   tree, and decision 28's fifth preference declares it absent rather than
   building one class of it. The outright stays in `FinancingRegister`,
   reported at UNDERSTAND with every unvalued instrument raised as a
   problem; when a mark plane is decided, it reads the register and does not
   re-derive the figure. No size reads the outright for the same reason: a
   size is set against equity, and equity carries nothing at a mark.
3. **A cross is priced only through its one route.** A currency with no pair
   against the base is priced through the one pair that joins it to a
   currency quoted directly. Both legs are quoted instruments, so this
   measures and invents nothing. Two routes are two figures for one rate,
   differing by the cross's basis; the register does not choose between them,
   and the currency stays unpriced, which `FxRates::to_base` refuses. A cross
   is never built on another cross.

## Consequences

- `R12-COVERAGE-5098`'s three gaps are each closed or declared, with the
  reason in this record.
- A lane that later wants a per-currency cash book derives it from the fills
  under a new ADR; it does not open a second writer.
- A lane that builds marking builds it for every class at once and reads the
  financing register for FX forwards and swaps.

## What it costs

- The platform cannot answer "how many euros does the desk hold" from a
  currency book; the answer is the net position in the euro pairs, read off
  the portfolio. A per-currency view is a derivation still to be written.
- An open forward's gain or loss is invisible to equity and to the risk
  monitor until a fill realises it, as an equity's is today.
- A currency two pairs reach is refused where a chosen route would have
  priced it; the desk sees a refusal it could have avoided by quoting the
  currency directly.

## What would make this wrong

- A conversion that is not a fill: a treasury sweep between currency
  accounts with no instrument behind it. None has a producer in the tree
  today; the first one needs a source of rates and a record, and it is a new
  ADR.
- A mark-to-market plane landing without reading the register: the outright
  would then be computed twice.
