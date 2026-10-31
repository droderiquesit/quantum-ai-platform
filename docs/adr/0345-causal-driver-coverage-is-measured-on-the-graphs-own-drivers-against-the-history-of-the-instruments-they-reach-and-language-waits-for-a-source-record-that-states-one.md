# ADR 0345: Causal-driver coverage is measured on the graph's own drivers against the history of the instruments they reach, and language waits for a source record that states one (amends ADR 0151 decision 12)

- **Status**: **Accepted**, 2026-10-31. Decided by lane L4-quant under the
  parent's 2026-10-31 delegation
  (`docs/blueprint/owner-decisions-2026-10-31.md`, decision 10), on the
  ruling for R12-EXPAND-6065
  (`docs/blueprint/lanes/wave1031-handoff-2026-10-31.json`, lane "L4b").
  The number is from L4b's block (decision 15). The delegation does not
  lift four limits:
  - no live-order path;
  - no real capital or money movement;
  - no weakened test or limit;
  - no new dependency.

  This decision needs none of them. Decision 10 records that the row was
  filed with the live-capital rows by mistake and needs no capital.
- **Date**: 2026-10-31
- **Amends**: ADR 0151 decision 12, its coverage-dimension clause only.
- **Keeps**: ADR 0151 decision 12's other clauses (the belief price level
  stays unbuilt; ADR 0346 is the record for that row).

## Context

The coverage detector (`qip_expansion::detectors::coverage`) raises a
source gap for a dimension value whose instruments have too little price
history. Its dimensions were asset class, venue and geography. ADR 0151
decision 12 kept language and causal drivers out because "language and
causal-driver inventories need a measured source".

For causal drivers that source already exists. The platform keeps a causal
graph (`WorldModel::causal`), each claim naming a cause and an effect, and
the sweep already reads it for the missing-link detector. A driver the
platform reasons through, whose effects it cannot price, is the same lack
as an asset class it cannot price. Nothing raised it.

For language there is still no source. No admitted record states the
language of what it carries, so a language dimension would count records
by a field nobody set.

## Decision

1. **`CoverageDimension::CausalDriver` is a coverage dimension.** Its key is
   `causal_driver:<cause>`, and its noun in an observation is "causal
   driver".
2. **A driver's instruments are the effects its live claims reach.**
   `causal_driver_universe` maps each `(cause, effect)` claim to the effect,
   counting only effects the catalogue's coverage map holds as instruments.
   An effect the platform does not trade (a rate, a factor, a commodity with
   no instrument) has no history to lack; reading it as uncovered would
   raise a gap no source could close. A driver that reaches no instrument is
   left out.
3. **The map is read each sweep, from the graph as it stands.** A claim is
   live when it was recorded by the sweep's instant and not retired by it.
   The catalogue's dimensions are fixed when the node starts; the graph is
   not, so its rows are added to the coverage map in `sweep_over` and are
   never stored in it.
4. **The share and the floor are the asset class's.** A driver is scored by
   the share of its instruments with price history, against the same
   `COVERAGE_FLOOR`, and raises one observation naming its dimension.
5. **Language stays unmeasured.** It becomes a dimension when a source
   record states the language of what it admits. Until then R12-EXPAND-6065
   stays PARTIAL on it.

## Consequences

- `ResearchLoop`'s sweep raises a coverage gap for a driver whose effects
  lack history. `evidence_gaps.rs`
  `a_causal_driver_whose_effects_have_no_history_becomes_one_coverage_gap_naming_its_dimension`
  drives it through `ResearchLoop::sweep`, with claims seeded through
  `Platform::claim_causal`.
- On a shipped deep brain it cannot fire yet. The only causal-edge writer a
  shipped binary reaches is the temporal-precedence discovery, which claims
  edges only between subjects that already have price history, and no app
  calls `Platform::claim_causal`. Every shipped driver's effects are
  therefore covered. The dimension is wired and waits for a writer that
  claims an edge into an instrument the platform cannot price.
- The ontology seeding (`sweep_ontology_over`) reads the static map, so it
  registers no causal-driver type. A driver is a claim in the graph, not an
  ontology type, and this decision does not make it one.

## What it costs

- One more source of gaps. A graph rich in drivers over a thinly priced
  universe raises a gap per driver, on top of the asset-class gap for the
  same instruments. The two name different things to research: the class
  asks for a source of prices, the driver for the instruments that driver
  moves.
- A sweep-time pass over the causal graph's edges: one filter and one map
  insert per edge. The graph has no edge cap (`CausalGraph::add` keeps every
  claim), so the pass grows with it. The map it produces is bounded by
  causes times instruments.

## What would make this wrong

- A measured case where driver gaps duplicate asset-class gaps so often
  that the desk spends its budget on the same instruments twice. Then the
  driver should be raised only for instruments its class does not already
  name.
- A source that does state language. Then language needs its own record,
  and this decision's fifth clause is superseded.
