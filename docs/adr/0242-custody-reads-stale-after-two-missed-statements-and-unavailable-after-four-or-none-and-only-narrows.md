# ADR 0242: Custody reads stale after two missed custody statements and unavailable after four or none since start; the node hands it to the cell, and it only narrows

- **Status**: **Accepted 2026-10-31 under the owner's 2026-10-31 delegation**,
  relayed by the wave parent to lane L5-EDGE (decision 10 of the 2026-10-31
  rulings, `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`).
  The limits of that delegation hold here. This record sets no risk number
  and changes no limit, threshold or limit set: the custody multipliers are
  the contract's existing `CUSTODY_STALE_MULTIPLIER` and
  `CUSTODY_UNAVAILABLE_MULTIPLIER`. It touches no paper-trading layer and
  adds no constructor to `Cell`.
- **Date**: 2026-10-31
- **Extends**: the §6.2 degradation table in
  `qip-contracts/src/degradation.rs`, whose custody row had multipliers and
  no reading.
- **Depends on**: R12-LEDGER-10185's `custody_statement`, which has not
  landed. See decision 5.
- **Rows**: R12-RES-7099.

## Context

R12-RES-7099 asks that a cell narrow on custody it cannot confirm. The
contract already prices that row: stale custody sizes at three quarters and
unavailable custody at one half. But no plane had a reading of custody.
`Cell::narrowing` said so in its own documentation: "Custody is not
recorded: a cell has no reading of it, and a row without a reading would be
a number nobody computed."

The ruling settled where a reading may come from, and where it may not:

- Custody is read from the simulated venue's own custody statement and from
  nothing else.
- No operator route sets it. A person typing "custody is fine" is not
  custody.
- No TransferGate input is added. That gate has no production caller, and a
  check nobody calls passes everything.

## Decision

1. **The reading is the newest custody statement's age, in statement
   periods.** A venue states custody at a declared period. Ages are judged
   against that period:
   - within two periods: **Fresh**;
   - older than two periods: **Stale**;
   - older than four periods: **Unavailable**.
   Two, because one missed statement is a late message and two is a pattern.
   Four, because by then the custody being read is older than any position
   the cell opened since.

2. **No statement since the node started is Unavailable, from the first
   pass.** A node that has never heard from custody cannot claim it is
   fresh. "Not yet" is not given a grace window, because a grace window is a
   number that widens.

3. **The node derives the reading and hands it to the cell. The cell
   records it and compounds it into pass sizing.**
   - The cell does not read statements. It gets the derived freshness, as it
     gets the journal pressure.
   - `Cell::narrowing` records the custody row only once a reading has been
     handed in. A cell nobody handed a reading to still records none, which
     is today's behaviour.
   - The pass compounds the custody row's multiplier beside the clock row's.

4. **It only narrows.** Fresh multiplies by one. Stale and Unavailable
   multiply by the contract's existing constants, never above one. Nothing
   here can enlarge an order, open a route, or move the ceiling.

5. **Armed only when a statement period is configured, until the statement
   exists.** R12-LEDGER-10185's custody statement has not landed, so nothing
   produces one. Arming the reading on every node would size every node at
   the unavailable multiplier for a statement no venue sends. That is
   restrictive, but it would be a degradation nobody could clear.
   - The node therefore arms the watch when a statement period is
     configured, defines the statement type it reads, and feeds the cell
     from it.
   - Wiring the simulated venue's statement to the watch is left for when
     LEDGER-10185 lands. The row's gap says so.

## What changes and what does not

- **Changes.**
  - A node with a custody period configured sizes at the custody row's
    multiplier while statements are late or absent.
  - The cell's narrowing reports a custody row whenever it was handed one.
- **Does not change.**
  - A node with no period configured, and every cell nobody hands a reading.
  - Every constant in the degradation table.
  - The `Cell` constructors.
  - Any limit.

## What it costs

- A configured node that never receives a statement trades at half size
  until it does. That is the intended reading of "none since start".
- Two periods of grace means a custody failure is sized for at most two
  periods before narrowing begins.

## What would make this wrong

- If a venue's statement period varies by design, so that a fixed multiple
  of a declared period flags a healthy venue as stale. The period would then
  have to be read from each statement, not declared once.
- If LEDGER-10185's statement carries an "as of" time different from when
  it arrived. The age must then be taken from the "as of" time, which is the
  conservative reading.

## Evidence

- `qip-edge-node/src/custody.rs` holds `CustodyStatement` and
  `CustodyWatch::freshness`.
- `Cell::observe_custody` and the custody row in `Cell::narrowing`.
- The pass's custody multiplier.
- Test: `with_custody_statements_stopped_the_cell_sizes_at_the_unavailable_multiplier`.
  Mutation: report Fresh.
