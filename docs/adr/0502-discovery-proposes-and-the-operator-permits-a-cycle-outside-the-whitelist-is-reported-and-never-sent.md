# ADR 0502: Discovery proposes and the operator permits: a cycle outside the whitelist is reported and never sent

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**,
  session t50-38 (wave580), implementing the owner's ruling on
  R12-MESH-5042 (`docs/blueprint/owner-decisions-2026-10-31.md`,
  ADR_THEN_IMPLEMENT: "Discovery proposes and the operator permits. The
  signed policy carries an operator-signed market reference, and the cell
  reports any cycle it finds outside the whitelist and sends nothing on
  it. Only the whitelist reaches the send path. The distributed half waits
  on a second node, which decision 5 keeps unbought."). An in-repository
  design decision: it adds an additive field to a signed slot and a record
  to the cell's journal. It widens nothing that trades, adds no dependency
  and touches no paper-trading layer.
- **Date**: 2026-11-13
- **Rows**: R12-MESH-5042 (stays PARTIAL; see "What this leaves open").
- **Title note**: the ruling file's shared title ends "and an inventory
  target may only be lowered". The skeptic on that ruling recorded that
  clause as a misread belonging to R12-MESH-5059's discussion. It is not
  decided here, and this ADR takes no position on inventory targets. The
  clause about the centre narrowing its whitelist to the cycles it
  discovers is R12-MESH-5059's to implement; this ADR states only what
  the cell does with a reference the centre ships.

## Context

ADR 0154 decision 1 refused discovery at the cell because "discovery adds
conversions nobody signed", and asked in "What would make this wrong" who
would sign discovery. The cell held no way to form a cycle outside the
whitelist anyway: a book is named by its own instrument id and carries no
from/to, so `registry_from_books` and `Installation::InstalledWithGaps`
(MESH-023) could report an unwhitelisted book side and could never join
two of them into a path. `WhitelistedConversion` was the only place a
from/to existed.

The requirement's own check is "a discovered cycle outside the whitelist
is reported" — reported, not traded — and its mutation is "drop the
whitelist edges and no opportunity is found".

## Decision

1. **The operator's market reference rides the signed whitelist slot.**
   `CycleWhitelist::market_reference: Vec<ReferencedMarket>` names, per
   book, its venue, venue class, base, quote and taker cost. Each book is
   stated once; both of its conversions follow from the convention
   `WhitelistedConversion` already documents (asks turn the quote into the
   base, bids the base into the quote), so a reference cannot state a
   book's two sides inconsistently. The field is skipped when empty, so a
   payload signed before it keeps its bytes, digest and signature; a cell
   built before it refuses a payload carrying it by `deny_unknown_fields`,
   the safe direction. It shares slot 8's digest, freshness and codec
   rather than adding a ninth `PolicyItem`, because the reference and the
   whitelist are one operator statement about one set of markets and two
   slots could go stale apart.
2. **The centre issues the reference from the operator's own markets.**
   `ArbitragePolicy::whitelist_for` emits one `ReferencedMarket` per
   `CentralConfig::arbitrage` market. The operator signs those markets by
   writing the policy; nothing the centre or the cell computes adds a
   market to the reference.
3. **The cell searches every referenced book and reports; it never sends
   on what it finds.** `ArbitrageDesk::with_discovery` gives the desk a
   second graph built from the reference (`graph_from_reference_in` in
   `qip-edge-node`, refused whole on the same terms as the whitelist: a
   venue outside the cell's list, a book quoting an instrument against
   itself, a cost outside `[0, 1)`, one venue stated as two classes, more
   edges than a pass walks). `ArbitrageDesk::discover` re-quotes it from
   the same books, scans it with the desk's own scanner and sizes, and
   keeps each opportunity with at least one trade edge whose venue, book
   and side the whitelist does not carry.
4. **The report is structurally unsendable.** What `discover` returns is a
   `DiscoveredCycle` — the path's edge labels, the edges outside the
   whitelist, and the priced net — with no leg, no plan and no
   `Opportunity`. `Cell::work` puts it on `WorkReport::discovered_cycles`
   and journals `Decision::CycleDiscovered { cycle, outside, net }`, a P2
   record (`event_fabric::writer::is_outcome` classifies it as not an
   outcome, because nothing happened at a venue). The send loop reads only
   the whitelist graph's `ScanReport`; there is no path from a discovered
   cycle to `Cell::place_cycle`.
5. **The distributed half is not built.** Scanning across nodes over the
   peer mesh (ADR 0116) needs a second node, which owner decision 5 keeps
   unbought (`docs/blueprint/lanes/l5b-peer-mesh-root.patch`).

## Evidence

- `cargo test -p qip-edge-node --test arbitrage`:
  `dropping_a_whitelist_edge_leaves_no_opportunity_and_the_reference_reports_the_cycle_with_that_edge_outside`
  and `a_discovered_cycle_outside_the_whitelist_is_reported_and_never_sent`.
- Mutations, each restored byte-for-byte: routing the reference graph into
  the desk's `scan` and `refresh` (discovery reaching the send path) fails
  the first test with "an opportunity was found through a conversion the
  whitelist does not carry"; dropping the outside-the-whitelist filter
  fails both tests' premise that a wholly whitelisted cycle is not
  reported; dropping the journal record fails the second with "the
  discovery was not journaled".

## What this leaves open

- **Distributed opportunities.** The requirement says "for both local and
  distributed opportunities". Only local is built (decision 5).
- **The reference equals the whitelist today.** Because `whitelist_for`
  emits every operator market as both reference and conversions, a
  deployed cell finds nothing outside the whitelist until R12-MESH-5059
  narrows the whitelist to discovered cycles, or an operator states markets
  for reference only. Neither is built here.
- **Nothing is deployed.** `execution_nodes_enabled = false` in dev and no
  node exists elsewhere, so no discovery record reaches a deployed log.

## What it costs

A second scan per pass on a cell whose payload carries a reference,
bounded by the scanner's `max_candidates` and by `MAX_CONVERSIONS` edges.
No new allocation persists between passes beyond the reference graph and
its per-edge read record, both fixed at installation.

## What would make this wrong

A path by which a `DiscoveredCycle`, or anything built from the reference
graph, reaches `Cell::place_cycle` or a `Placer`. A reference entry the
cell adds itself. A reference read past an entry the installer should have
refused.

## Consequences

The operator sees, in the journal, every profitable cycle its own markets
form that the whitelist does not permit, with the conversions that would
have to be permitted, and decides. The cell's send path is unchanged.
