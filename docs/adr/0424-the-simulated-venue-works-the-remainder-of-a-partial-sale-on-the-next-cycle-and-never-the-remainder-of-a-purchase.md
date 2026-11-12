# ADR 0424: The simulated venue works the remainder of a partial sale on the next cycle, and never the remainder of a purchase

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session fw-l-01. It changes what the
  simulated venue does with an order it has already accepted; it adds no
  venue, no order path and no live capability, and touches none of the three
  paper-trading layers.
- **Date**: 2026-11-12
- **Reads**: `SimulationSettings::immediate_fill_fraction`
  (`qip-execution-engine/src/broker.rs`), whose own documentation says the
  rest of an order "is worked" — and until this ADR nothing worked it.
- **Rows**: R12-ASSET-4071.

## Context

R12-ASSET-4071 requires every desk position to walk discover to attribute,
with nothing reaching attribute unsettled. The kernel's register
(`qip-kernel/src/position_life.rs`) does that walk at the desk's booking
seam, but an independent check on 2026-10-25 found that on the shipped
simulator no position ever reaches `Exit`. The venue fills
`immediate_fill_fraction` (0.6) of what remains on each submission and the
rest stays open on the order manager's book forever: a sale of the whole
holding leaves four tenths of it, a second sale of that leaves four tenths
again, and the one-unit lot grid refuses the next order once the remainder is
fractional. The only way the register's own test reached flat was to
oversell into a short with a borrow reading only a test supplies.

Two remedies were named: a production borrow source, or a fill rule that can
reach flat. A borrow source is a data source, and `qip-data-finder` must
evaluate its licensing posture before one is used; that is not a decision a
lane can take. The fill rule is in-repository simulator semantics and is.

## Decision

1. **An accepted order's remainder is worked by the venue that accepted it,
   through a new `Broker::work` port method.** The default does nothing: a
   real venue's working is its own, and its fills arrive the way it reports
   them. `SimulatedBroker::work` fills the requested quantity of the
   remainder in full, at the venue's price for that quantity at the instant
   it is worked, with commission, if a limit order's limit is still reached.
   It draws nothing from the venue's random stream — the order was accepted
   when it was submitted, and a second rejection draw would both invent a
   refusal the venue never made and shift every later submission's draw, so
   a replay of the same orders fills the same way.

2. **Only a sale is worked, and only down to flat.** At the start of each
   ACT stage, before any new order, the kernel asks the order manager to work
   every open sell order's remainder up to the long the desk holds in that
   instrument (`OrderManager::work_sales`). A purchase's remainder is not
   worked, and neither is the part of a sale that would extend or open a
   short. The asymmetry is the point: working a remainder skips the
   pre-trade check that ran when the order was submitted, which is sound
   only for a fill that reduces exposure. A purchase filled a cycle later
   adds risk against limits read a cycle earlier; a sale toward flat removes
   it. A sale into a short went through the borrow gate for the quantity it
   was submitted with, and that gate is not re-run here, so the short part
   waits.

3. **A halted scope or an autonomy level that does not permit execution
   works nothing.** The kill switch and the autonomy check are the same two
   the order manager runs on submission. A stopped platform stays stopped
   even for an order it accepted before it stopped. A venue withdrawn on
   feasibility evidence, and any venue that is not simulated, works
   nothing: the second is belt and braces beside the port default, so that
   no remainder can reach a venue the paper ceiling forbids even if a
   future adapter overrides `work`.

4. **A worked fill is booked through the same seam as any other.** It is
   ledgered, journaled, captured as `Filled` on the cycle's decision record
   and booked through `Platform::book_desk_fill`, which advances the
   position's `LifeStage`. It is not counted as an order submitted: the
   order was submitted once.

## What it costs

- On the default simulated venue a sale of a whole long reaches flat on the
  next cycle, so the position reaches `Exit`, is settled on the venue's
  calendar and is attributed by the LEARN after settlement — the walk the
  row requires, on the default path, with no borrow reading.
- A sale's fill now arrives in two parts on two cycles. Anything that read
  "a partial sale stays partial" was reading the gap this ADR closes.
- A purchase is unchanged: 0.6 now, the rest open and unworked, exactly as
  before, so every result that sized against the shipped fill fraction for
  an opening order reads the same.

## What would make this wrong

A desk whose sales are large against the venue's book would be credited a
full fill one cycle later at a price that ignores the impact of the first
part. The venue's square-root impact is charged on the worked quantity
alone, not on the cumulative order; a desk relying on the simulator for
large unwinds should read the worked fill's price as optimistic by that
interaction.
