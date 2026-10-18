# ADR 0126: The signed capital envelope is the one downward capital authority; it carries a drawdown bound, and a hedge plan reaches a cell only as a narrowing it may refuse

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). Every change it decides tightens what
  a cell may do; none widens a grant, opens an order seam or touches a
  paper-trading layer.
- **Date**: 2026-10-18
- **Extends**: ADR 0008 (cells decide alone, on capital granted in advance),
  ADR 0110 (the envelope is signed under the `envelope` artefact class),
  ADR 0123 decision 6 (the grant's funding is stated from the envelope).
- **Keeps**: ADR 0003, ADR 0109 (no dependency), the spine's
  `CapitalGrantFrame` as the only frame that carries capital down.

## Context

Two contracts describe the same capital and only one of them reaches a cell.

- `qip_contracts::CapitalEnvelope` (`qip-contracts/src/capital.rs`) is what
  the centre signs, `qip_mesh::spine::CapitalGrantFrame` carries down, and
  `qip_edge::envelope::VerifiedEnvelope` verifies before `admit` and
  `check_exposure` use it. It holds a gross limit, an order limit, a loss
  limit, venues, a window, the approver's signature and, since R12-GOV-6120,
  optional `ExposureBounds` (participation in visible depth, leverage over
  remaining equity, the backing capital). Its loss limit fires because the
  cell books its own realised losses (`qip-edge/src/realised_loss.rs`,
  proven by `qip-edge/tests/drawdown.rs`).
- `qip_contracts::capital::CapitalGrant` (same file, `CapitalGrantFields`)
  is the §24.9/§27 statement: owner, mandate, collateral class, leverage,
  risk budget, liquidity reserve, `max_drawdown_bp`, ADV share, expiry. It
  has one consumer, the ledger's funding statement, and
  `Platform::fund_capital_grant`'s documentation says how three of its
  fields are *written* from the envelope because the envelope does not state
  them: the reserve is zero, the drawdown stop is the loss limit over the
  capital, and the ADV share is the whole of it.

So R12-CONTRACT-7066 ("the grant exists between brains") is PARTIAL because
the cell enforces the envelope and the grant's reserve, drawdown stop and
ADV share are restatements; R12-CAPITAL-4088 because no peak-to-trough bound
distinct from the loss limit exists; and R12-CAPITAL-10098 because
`qip_contracts::control::HedgePlan` ("a plan, not an order: nothing here
reaches a venue") stays central and no spine frame carries it.

Locate each with
`grep -n 'pub struct CapitalEnvelope\|pub struct ExposureBoundsFields\|pub struct CapitalGrantFields' backend/crates/libs/qip-contracts/src/capital.rs`,
`grep -n 'pub struct CapitalGrantFrame' backend/crates/services/qip-mesh/src/spine.rs`,
`grep -n 'pub struct HedgePlanFields' backend/crates/libs/qip-contracts/src/control.rs` and
`grep -n 'fn fund_capital_grant' backend/crates/runtime/qip-kernel/src/platform.rs`.

## Decision

1. **The envelope is the one downward capital authority, and it is not
   joined by a second.** Sending the `CapitalGrant` down beside the envelope
   would give the cell two signed claims about the same capital, and the
   repository's own principle says the louder one will be wrong. The grant
   stays the ledger's statement *of* the envelope (ADR 0123 decision 6). What
   R12-CONTRACT-7066 asks the grant to carry to the cell is carried by adding
   it to the envelope, through the same additive-and-validated path
   `ExposureBounds` took.
2. **The envelope gains a drawdown bound** (R12-CAPITAL-4088): an optional
   `max_drawdown` in the account currency inside a new versioned bounds
   block, refused at construction and at decode when it is not positive or
   exceeds the loss limit (a drawdown stop looser than the loss limit is
   not a bound). The cell measures it from its own `RealisedLedger`: the
   strategy's realised P&L high-water mark within the grant, less its
   current realised P&L. `admit` refuses once the drawdown reaches the
   bound, beside the loss-limit refusal, journaled under its own gate.
   A grant without the block behaves as today; a cell configured to require
   it refuses an envelope that lacks it, as
   `CellConfig::with_exposure_bounds_required` does for exposure.
3. **The reserve and the ADV share follow the same path when a producer
   states them.** The bounds block reserves both fields; the centre fills
   them only when a sizing input actually states a reserve or an ADV
   bound, and `fund_capital_grant` then reads them instead of writing
   zero and the whole. Until then the grant's documentation stays the
   honest record of what is written rather than observed.
4. **A hedge plan travels down as a narrowing frame** (R12-CAPITAL-10098).
   A new spine frame, `HedgeFrame`, carries a signed `HedgePlan` for one
   cell, signed under the `envelope` class because it is capital authority.
   It may only **reduce** what the cell may hold: per instrument, a target
   net exposure toward zero inside the envelope's existing bounds and a
   deadline. A cell applies it by refusing new exposure that moves away from
   the target and by preferring orders that move toward it inside its
   existing limits; it never authorises an order the envelope would refuse,
   never raises a limit, and never names a venue the envelope does not. A
   frame for another cell, past its deadline, or naming an exposure the
   envelope does not grant is refused and journaled; the cell keeps deciding
   alone (ADR 0008) and a cell that never hears the frame is exactly as safe
   as today.
5. **Every new field is additive under the schema lock.** The envelope's
   signed bytes stay stable for an envelope without the new blocks, so a
   cell on the old build verifies an old-shape envelope and refuses a new
   shape rather than ignoring the bound.

## Consequences

- One contract governs a cell's capital, and the ledger's statement of it
  becomes more observed and less written as fields arrive.
- Code slices left: the bounds block in `capital.rs` with refusal and
  round-trip tests, the drawdown measurement and `admit` refusal in
  `qip-edge` with a test in `qip-edge/tests/envelope_exposure.rs` or
  `drawdown.rs` that a strategy past its drawdown is refused while under its
  loss limit (CAPITAL-4088); `fund_capital_grant` reading the block
  (CONTRACT-7066, with a `downward_signed_path.rs` test that a grant with a
  drawdown stop is refused at the cell past it); `HedgeFrame` on the spine,
  the cell's acceptance rule, and a `qip-edge-node/tests/degraded.rs` test
  that a cell applies a hedge target and refuses an order moving away from
  it (CAPITAL-10098).

## What it costs

- A drawdown measured on realised P&L only misses an open position's
  unrealised loss. The cell has no independent mark it trusts for every
  instrument, and marking to its own feed would let a stale feed hide a
  loss. The loss limit and the exposure bounds still bound the open book;
  the drawdown bound is the second line, not the only one.
- A hedge frame is one more signed artefact on the downward path, one more
  refusal gate in the cell, and one more thing the centre and the cell can
  disagree about. It can only narrow, so a disagreement costs trades, not
  capital.
- The envelope's contract grows twice in one quarter; every reader of the
  signed shape needs the additive rules applied exactly or a verification
  fails across a version boundary.

## What would make this wrong

- If a live-capital record (ADR 0108) is ever accepted and requires the grant
  itself to be the signed instrument a custodian or regulator reads, decision
  1's "one authority" would have to name the grant rather than the envelope;
  this record does not anticipate that and keeps paper semantics.
- If hedges need to *add* exposure in a cell (buying protection rather than
  reducing a position), decision 4 cannot express it, and the right answer is
  a new envelope from the centre, not a hedge frame that widens.
- If cells observed in practice routinely breach the drawdown bound only
  because realised P&L is lumpy (one large closing trade), the measurement
  needs smoothing by an ADR, not a looser bound.

## Rows

R12-CONTRACT-7066, R12-CAPITAL-4088, R12-CAPITAL-10098. Each stays PARTIAL
until its code slice runs on a default path and a mutated test proves it.
