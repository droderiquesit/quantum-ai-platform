# ADR 0396: Digital assets settle on paper through a simulated chain: proceeds wait on an operator-declared finality depth, a bridge is a declared delay and fee, and nothing signs or broadcasts a transaction

- **Status**: **Accepted 2026-11-11 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Batch l6-d2-01 writes
  it. The title is the one the owner ruling of 2026-10-31 gave this row
  (ADR_THEN_IMPLEMENT). The limits of the delegation hold:
  - it sets no figure: every depth, interval, delay and fee is declared by
    an operator, and the code holds no default for any of them;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency and no crate edge (`qip-kernel` already depends
    on `qip-chain`);
  - it can only delay a release of proceeds and withhold a fee.
- **Date**: 2026-11-11
- **Extends**: ADR 0155 section 17, which kept `DexVenue` a feasibility and
  cost model and deferred chain settlement. It amends no clause: a
  digital-asset order still uses the same simulated gateway as every class,
  and `qip-kernel` still takes no `qip-brokers` dependency.
- **Rows**: R12-COVERAGE-5103.

## Context

A digital-asset sale's proceeds were released on the venue's settlement
calendar, as an equity's are (`Platform::register_settlement`). A chain does
not settle on a calendar. Proceeds are usable when the including block is
buried to the depth the holder requires. A transfer across a bridge arrives
later still, and smaller by the bridge's fee. Survival sizing could
therefore count proceeds that a real chain would not yet have made final.

## Decision

1. **Terms are declared, per venue.**
   `qip_kernel::chain_settlement::ChainSettlementTerms::declare` takes a
   finality depth (`qip_chain::Confirmations`), a block interval and an
   optional `DeclaredBridge` (a delay and a `qip_chain::FeeBps`). An
   operator hands them to `Platform::declare_chain_settlement(venue, terms)`.
2. **Refused, not defaulted.** A depth of zero is refused: the chain head is
   `Confirmations::AT_RISK`, and proceeds a reorg can void are not settled.
   A block interval that is not positive and a negative bridge delay are
   refused for the same reason.
3. **The release instant.** For an instrument classed `DigitalAsset` at a
   venue with declared terms, proceeds are available at
   `fill + depth × block_interval + bridge delay`. Until then they are
   withheld from survival sizing through the same register that withholds a
   T+n equity sale.
4. **The bridge fee.** The simulated broker credits the whole sale. No
   ledger kind books a bridge's cut, and this ADR adds none. The fee is
   therefore withheld from survival sizing for good
   (`Platform::chain_bridge_fees`), and is part of
   `Platform::unsettled_sale_proceeds` at every instant.
5. **Undeclared stays as it was.** A venue with no declared terms is given
   none. A digital asset there settles on the venue's calendar, exactly as
   before this ADR. Every other class is unaffected at every venue.
6. **Nothing signs or broadcasts.** The module is arithmetic over declared
   terms. It reads no chain, holds no key and performs no I/O.
   `qip_chain::BridgeRoute` and `BridgeLedger` remain the fuller model and
   are not driven by it.

## What changes and what does not

- New: `qip-kernel/src/chain_settlement.rs`; two fields and three methods on
  `Platform`; one branch in `Platform::register_settlement`.
- Unchanged: the simulated gateway, every limit and gate, the ledger's
  operation kinds, the three paper-trading layers.
- No shipped binary declares terms. The figures are an operator's to give,
  so until one does, the platform behaves as it did.

## What it costs

The chain is a clock, not a chain. A reorganisation that would orphan the
including block is not simulated here; `qip-chain`'s `ChainState` models
that for observed blocks, and no observed block backs a paper fill. The
declared depth stands in for the risk it is declared against.

## What would make this wrong

- A ledger kind for a bridge fee is accepted. Then the fee is booked there
  and the permanent withholding here is retired, by a new ADR.
- The desk takes real custody of a digital asset. That is a live path, and
  the paper boundary refuses it before this ADR is reached.

## Evidence

- `backend/crates/runtime/qip-kernel/src/chain_settlement.rs`.
- `backend/crates/runtime/qip-kernel/src/platform.rs`
  (`Platform::declare_chain_settlement`, `Platform::register_settlement`,
  `Platform::chain_bridge_fees`).
- `backend/crates/runtime/qip-kernel/tests/chain_settlement.rs`: three
  tests, each listing in its doc comment the mutations that fired.
