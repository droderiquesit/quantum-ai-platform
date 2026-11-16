# ADR 0714: Promoted world knowledge stays in the central brains with no graph database, and the deep brain is the strategic forecast service

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-WORLD-10042, R12-WORLD-8129, R12-WORLD-9025
- **Keeps**: ADR 0024, ADR 0113, ADR 0191, ADR 0214, ADR 0638.

## Context

R12-WORLD-10042 asks for promoted world knowledge in Spanner Graph, in a
database apart from the ledger, so graph load cannot touch money truth.
R12-WORLD-8129 asks for a graph writer fed by world-model workers whose
queries serve cognition and never hot execution. Both rest on a store no
Accepted record admits. ADR 0113 admitted Spanner for the ledger only and
says Spanner Graph is not admitted by it. ADR 0638 made BigQuery the one
evidence store agents read. ADR 0191 placed the world model in the central
brains and refused a separate world-model service before deployment, because
a new database is billed infrastructure.

Today the graph is the in-memory `qip-world-model` graph. It shares no
database with the ledger, so ledger commits cannot be slowed by graph load.
Edge crates are barred from the world-model crates, and an acceptance test
holds that bar.

R12-WORLD-9025 asks that the weeks-to-years forecasts run as a separate
service whose outage only accumulates backlog. The lattice backlog and the
deep brain's outage gate exist (ADR 0214). What was open was owner decision
O24: whether to split a new service. The owner ruled on 2026-10-31 that no
new service is deployed and the deep brain, already its own Cloud Run
service, is the strategic forecast service.

## Decision

1. **R12-WORLD-10042 (need a figure).** No graph database is declared. The
   requirement is met by separation in the present design: promoted
   knowledge lives in the central brains' memory, apart from the ledger
   database. The row reopens only through a record that states two figures,
   neither chosen here: the measured graph query load, and a monthly
   ceiling for the store. Until both exist the row stays MISSING and
   nothing is added to Terraform.
2. **R12-WORLD-8129 (decline).** No Spanner Graph writer is built. Writing a
   client against a store no record admits would also need its own
   dependency decision under ADR 0109. The edge-isolation half stays
   enforced by the existing acceptance test, and nothing here relaxes it. A
   later record that admits the store must also extend that test so it
   refuses any edge crate that reaches the writer.
3. **R12-WORLD-9025 (build).** The deep brain is the strategic forecast
   service. Only it serves the weeks-to-months and quarters-to-years
   lattice rows. The fast brain never asks one. No separate strategic
   service, image, identity or minimum instance is added. The outage
   behaviour stays as ADR 0214 set it: the deep brain's own readiness is the
   signal, the questions wait in the bounded backlog, and no stage of
   execution depends on them. The work is a test proving the fast brain's
   cycle holds no strategic row and that a halted deep brain leaves an
   execution cycle running.
4. **Nothing in this record is built or tested by it.** It touches no risk
   limit, no capital or budget control, no paper-trading layer, and adds no
   dependency or billed resource.

## What it costs

- The graph is lost on restart and relearned. ADR 0191 records that no
  replay rebuilds it. Its per-domain snapshots make the loss visible rather
  than curing it.
- Graph load and cognition share a process with other central work. That is
  the price of keeping a database out of the bill until a load is measured.
- The strategic and the deep-brain workloads scale together. They cannot be
  sized independently.
- Rows 10042 and 8129 stay open and unverified. This record does not close
  them.

## What would make this wrong

- A measured graph load that exceeds what the deep brain's memory holds, or a
  restart cost the snapshots do not cover. That is the evidence ADR 0191 and
  decision 1 above wait for.
- Strategic forecasting load that starves the deep brain's other work in a
  way the backlog bound does not contain. Then the separate service the
  owner declined on 2026-10-31 would need its own record and a priced
  ceiling.
- A fast brain path that turns out to call a strategic row. Decision 3's
  test would fail, and the fix is in the fast brain, not in this record.
