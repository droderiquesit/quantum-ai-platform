# ADR 0719: A private or illiquid position enters the book by its own route, and the five-day liquidity floor is not relaxed for it without the owner

- **Status**: **Proposed 2026-11-16, for the owner.** Decision 2 concerns a
  risk limit (the `MinLiquidity` floor) and decision 1 would amend Accepted
  ADR 0183. Neither is within the delegation of 2026-11-15. No paper-trading
  layer is touched, no dependency is added, and no figure is chosen.
- **Date**: 2026-11-16
- **Rows**: R12-ASSET-1015, R12-COVERAGE-5105

## Context

R12-ASSET-1015 asks that each asset class has an adapter and a lifecycle
engine, and that an unregistered class is refused. The registry maps every
class to an engine and `Platform::private_stage` now reads a commitment's
stage through it. What remains, per the register, is how a private, illiquid,
tokenized or physical position enters the book. A closure that refused a desk
venue fill on those classes was built and withdrawn, because ADR 0183 presumes
a private fund is sized by construction and held through a desk fill, and
three tests that hold a private fund would stop opening one.

R12-COVERAGE-5105 asks that private and illiquid investments are supported.
`PrivateMarket` and `RealAsset` are registered, and a private fund and a real
asset assemble on paper, but only under a limit set that no shipped root
loads. Every root runs `LimitSet::conservative_default` unless
`QIP_RISK_LIMITS_PATH` names a file, and a file cannot drop a shipped control.
The shipped `MinLiquidity { days: 5.0 }` floor refuses both orders. The
register records that closing the row needs a risk-domain decision and the
owner's sign-off on how that floor applies to classes that are not
continuously priced.

## Decision

1. **R12-ASSET-1015, owner.** How a non-listed position enters the book
   changes ADR 0183's presumption that a desk fill holds a private fund. The
   shape this record proposes, for the owner to accept or replace: a private
   or real position enters by commitment and capital call, a tokenized one by
   confirmed transfer, and a physical one by shipment, each through the
   adapter its class names and never through a venue order book. A venue fill
   on those classes would be refused by the order path. This amends ADR 0183,
   which a later record must name decision by decision. Until the owner
   decides, ADR 0183 stands, the desk-fill route stays, and no refusal is
   built. The adapter branch and its refusal test follow only after the
   amendment is accepted.

2. **R12-COVERAGE-5105, owner.** Whether and how the five-day liquidity floor
   applies to non-continuously priced classes is a financial risk limit. It is
   not decided here and not weakened here. The floor stays as shipped on every
   path. It is not relaxed, scoped out by class, or made loadable from a file
   as a side effect of this record. The row stays PARTIAL.

3. **R12-COVERAGE-5105, figure.** If the owner chooses to treat these classes
   differently, the figures needed are: the liquidity horizon, in days, that
   fits a class with no continuous price; whether it is a floor or a
   per-class bound; and the share of capital these classes may hold. None is
   chosen here. Any figure the owner gives is applied as a new, narrower limit
   beside the existing floor rather than in place of it, and it is recorded
   as provisional until measured.

4. **Quantum scope, decline.** Neither row requires a quantum family. Nothing
   here adds one, and ADR 0150 stands.

## What it costs

Both rows stay PARTIAL until the owner answers. A private fund and a real
asset cannot be traded on paper under the default limit set, which is the
intended behaviour for a desk that has not agreed to hold them. The existing
test that trades them under a private-holdings limit set remains evidence of
assembly only, not of the default path. The wait is the price of not letting a
lane edit a risk limit.

## What would make this wrong

If the owner holds that the registry and the private-stage reader already meet
R12-ASSET-1015's check, then decision 1 asks a question nobody needs answered
and the row should be regraded instead. If the floor is judged to protect
capital identically for every class, decision 2 is correct and the row should
be declined rather than left open. If a venue fill on these classes proves to
be the only route a desk ever uses, the commitment route proposed in decision 1
would be built and never reached, which is the defect ADR 0430 describes for a
control that cannot fire.
