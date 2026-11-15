# ADR 0632: The forecast lattice's row signals are set every UNDERSTAND pass from what the world holds at the pass

- **Status**: **Accepted 2026-11-15 under the owner's 2026-10-31 delegation**
  (ruling 14, ADR_THEN_IMPLEMENT), wave b52 lane 16. It narrows ADR 0151
  decision 6's clause that the composition root sets the signals, which no
  composition root called. It adds no dependency or figure and touches no
  paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-WORLD-3085, R12-WORLD-3088 (both stay PARTIAL).

## Context

`LatticeStage::set_signals` had no caller. On every deployed path the
seconds-minutes flow member therefore stated the level and the
quarters-years structural member read no claim, while each member's own
unit test passed. A member nothing feeds is a control that reads as
protection and cannot fire.

## Decision

1. `Platform::understand_now` sets the lattice's signals on every UNDERSTAND
   pass, before the lattice runs: live causal claims and the pending
   catalyst from `signals_from_world` (point in time at the pass), and the
   flow imbalance below. The stage consumes them in that pass, so a pass with
   none states none.
2. A catalyst or a live causal claim may only **widen** its row. The flow
   member shifts the mean by the imbalance and widens the row; no signal
   narrows one.
3. The flow map reads only R12-WORLD-3046's signed aggressor flow: the paper
   feed's own price-forming prints that state an aggressor. Nothing guesses a
   side. Because the feed is the paper simulator's, every flow so read is
   simulated; the platform has no other source.
4. `qip_kernel::lattice_stage::flow_imbalance_by_subject` is volume-weighted
   across venues, and a subject with no volume is absent, never zero.
5. The journaled `LatticeReview` carries `signals_read` (defaulted, skipped
   when empty), so a replay from the log alone shows what each pass was
   handed.

## Not decided

Propagation speed and liquidity outputs of the seconds-minutes row, scenario
generation (capital-cycle, policy, technology, geopolitical) for the
quarters-years row, and any downstream gate on `PromotionTarget`. Rows stay
PARTIAL for them.

## What it costs

- The lattice now depends on the federation step having run earlier in the
  same UNDERSTAND pass: the flow map is taken inside `understand_federation`
  and read by the lattice afterwards, so reordering those two steps would
  silently hand the flow member the previous pass's map, or an empty one on
  the first pass.
- Every pass reads the world model's live causal claims into each target
  subject and the macro surprises for the one pending catalyst, and sums the
  cycle's flow per subject. This record holds no measurement of that work.
- The flow member only ever sees the paper simulator's own prints, so every
  flow signal is simulated. R12-WORLD-3085 and R12-WORLD-3088 both stay
  PARTIAL for what "Not decided" lists.
- `signals_read` adds a field to the journaled review, so every pass that
  read any signal writes a slightly larger record; a pass that read none
  writes nothing extra.

## What would make this wrong

- A lattice pass whose journaled `signals_read` is empty while the world
  model held a live causal claim into one of the pass's target subjects or a
  pending catalyst at that instant, which
  would show the wiring has drifted from the world the pass claims to read.
- A second source of signed aggressor flow, such as a real venue feed,
  because decision 3 assumes the paper simulator is the only one.
- A forecast row that needs a signal to narrow its interval, which decision 2
  forbids, or evidence that widening on every claim leaves the intervals too
  wide to score against the calibrator.
- A composition root that does begin setting the signals itself, leaving two
  writers of the same state.

## Evidence

`qip-kernel/tests/lattice_signals_wired.rs`, each mutation-verified (see the
commit).
