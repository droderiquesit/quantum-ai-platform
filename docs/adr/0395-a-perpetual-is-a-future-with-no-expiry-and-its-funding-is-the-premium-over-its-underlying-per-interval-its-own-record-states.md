# ADR 0395: A perpetual is its own instrument type with no expiry; its funding is the premium of its mark over its underlying's price, per the interval its own record states, and no venue-published funding rate is read

- **Status**: **Accepted 2026-11-11 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Batch l6-d2-01 writes
  it. The limits of that delegation hold:
  - it sets no risk number and changes no limit, threshold or limit set;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency and no crate edge;
  - it books nothing to the ledger.
- **Date**: 2026-11-11
- **Extends**: ADR 0153 decisions 3 and 7 (a modelled figure is not booked
  to the ledger as a fact). It amends no clause of any accepted ADR.
- **Rows**: R12-COVERAGE-5102.

## Context

R12-COVERAGE-5102 asks that options, futures and perpetuals are supported
with margin, greeks and settlement models. The row stopped on three
questions, recorded in its gap on 2026-11-11:

1. The object model had no perpetual. `InstrumentType` had `Future`, and
   `FutureDetails` demands an expiry.
2. The only funding rate in the tree is the cell's
   `qip_edge::funding::FundingRate`, whose own doc leaves its period "to the
   caller to track".
3. No source supplies a funding rate to the centre.

## Decision

1. **Representation.** A perpetual is `InstrumentType::PerpetualFuture`,
   classed `Derivative`, carrying `Extension::Perpetual(PerpetualDetails)`.
   `PerpetualDetails` has no expiry field. It is not a `Future` with a
   far-off date: the kernel reads `FutureDetails::expiry` into its contract
   expiries, and an expiry nobody agreed would be acted on as a real one.
2. **Period.** The funding interval is the contract's own term,
   `PerpetualDetails::funding_interval_hours`, stated by whoever lists the
   instrument, as a repo's rate is its own. The code that values funding
   holds no interval constant. An interval of zero is refused.
3. **Source.** No venue-published funding rate has a producer in the tree,
   so that arm is declared absent and is not built on an invented input
   (decision 28, fifth preference). Funding is derived from the two records
   the universe does hold: `rate = (mark − index) / index`, where `mark` is
   the perpetual's price and `index` is its underlying's price. The payment
   one long contract makes one short contract at an exchange is
   `rate × index × contract_size`. Positive means the long pays.
4. **Where it runs.** `qip_kernel::financing::FinancingRegister` funds every
   perpetual at assembly, beside FX forwards and repos. The UNDERSTAND stage
   reports the count and raises every perpetual it could not fund as a
   stage problem naming the missing record.
5. **What is not booked.** The derived funding is a model's figure. It is
   reported and is not posted to the ledger, for the reason ADR 0153
   declines a modelled margin there: the survival governor reads the ledger
   back as fact. Booking waits on a venue-published rate.
6. **Permission.** A perpetual is a derivative first. `Platform::order_for`
   refuses it before an order exists until the desk holds the
   `PerpetualFuture` permission. A grant for `Future` does not admit it.

## What changes and what does not

- `qip-financial` gains one instrument type and one extension. Both are data.
- `qip-kernel::financing` gains `PerpetualFunding` and `fund_perpetual`.
- The cell's `FundingState` is unchanged. It still holds what it is sent,
  and nothing in the tree sends it a rate.
- No limit, gate, ledger kind or paper-trading layer changes.

## What it costs

The derived rate is the premium alone. A venue's published rate also carries
an interest component and a clamp, and each venue sets its own. Until a
source supplies the published rate, the figure here will differ from a
venue's by those terms. That is why it is reported and not booked.

## What would make this wrong

- A licensed source of venue funding rates enters the tree. Then the
  published rate is the fact, the premium becomes a check against it, and a
  new ADR decides the ledger kind that books it.
- A perpetual is quoted in a currency other than its underlying's. The
  register refuses that case today; it does not convert.

## Evidence

- `backend/crates/libs/qip-financial/src/asset_class.rs`
  (`InstrumentType::PerpetualFuture`).
- `backend/crates/libs/qip-financial/src/extensions.rs` (`PerpetualDetails`).
- `backend/crates/runtime/qip-kernel/src/financing.rs` (`fund_perpetual`,
  `PerpetualFunding`).
- `backend/crates/runtime/qip-kernel/tests/financing.rs`
  `a_perpetual_assembles_is_funded_from_its_premium_and_trades_on_paper_only_once_granted`,
  with the mutations that fired listed in its doc comment.
