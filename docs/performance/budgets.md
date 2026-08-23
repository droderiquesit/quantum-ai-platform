# Performance budgets

Every number in this document was measured by
`crates/tests/qip-acceptance/tests/performance.rs`, on the machine that ran it,
in one thread, with the fixture built before the clock started. Run
`make e2e`'s sibling — `cargo test --release -p qip-acceptance --test
performance -- --nocapture` — and the same lines print again.

A budget nobody measured is a wish, and a document full of wishes is worse than
an empty one, because somebody will design against it.

---

## 1. What has not been measured

First, because it is the part that gets skipped.

**No end-to-end latency has been measured.** Not wire-to-wire, not
tick-to-order, not cross-region, not anything involving a venue. This build has
no venue transport at all: the source probe, the order gateway, the managed
trainer and the quantum device are ports, and each reports itself unavailable
rather than pretending. There is therefore no figure in this repository for how
long the platform takes to react to a market, and any such figure quoted
anywhere is unmeasured and should be treated as fabricated.

**The figures below are per-stage, in-process arithmetic.** Each times one
stage in isolation with its inputs already in memory. A real path adds
decoding, sequencing, book contention, the journal, and — above all — a
network. None of that is here.

**Microsecond-class figures apply only to a colocated path this build does not
have.** A sub-microsecond `capital admit` is a statement about a comparison of
two `Decimal`s. It says nothing about a decision that had to cross a datacentre
first, and reading it as though it did is the specific overclaim this section
exists to prevent.

**Nothing here is a service-level objective.** These are regression floors. The
assertions in the test are loose enough that only a real regression — an
accidental clone per message, a linear scan where there was a lookup, a graph
recomputed per tick — trips one.

---

## 2. Measured, release profile

`cargo test --release`, single-threaded, one machine. Throughput is per core.

| Stage | Per operation | Throughput | What is being timed |
|---|---:|---:|---|
| Normalisation of a bar | 0.31 µs | 3.27 M/s | symbol mapping, unit conversion, quality stamp |
| Book apply (L2 level set) | 0.04 µs | 24.0 M/s | one level replaced in a venue book |
| Feature ingest + evaluate | 3.29 µs | 0.30 M/s | six features over a graph, dirty-marked and recomputed |
| Strategy run (19 nodes) | 0.58 µs | 1.72 M/s | one compiled strategy against one feature vector |
| Arbitrage scan (3 nodes, 3 edges) | 5.84 µs | 0.17 M/s | search, price, plan a triangular cycle |
| Capital admit | 0.008 µs | 122 M/s | envelope bound check against utilisation |
| Pre-trade risk (5 limits) | 0.68 µs | 1.47 M/s | the deterministic limit set |
| Order construction + validate | 0.14 µs | 7.08 M/s | building one order and checking it |

## 3. Measured, debug profile

`cargo test` builds unoptimised. A figure quoted without its profile is not a
figure, so both are here — and the ratio is the reason the distinction matters.

| Stage | Per operation | Throughput | Debug / release |
|---|---:|---:|---:|
| Normalisation of a bar | 0.96 µs | 1.05 M/s | 3.1× |
| Book apply (L2 level set) | 0.10 µs | 10.5 M/s | 2.3× |
| Feature ingest + evaluate | 11.65 µs | 0.09 M/s | 3.5× |
| Strategy run (19 nodes) | 1.92 µs | 0.52 M/s | 3.3× |
| Arbitrage scan (3 nodes, 3 edges) | 21.66 µs | 0.05 M/s | 3.7× |
| Capital admit | 0.065 µs | 15.4 M/s | 8.1× |
| Pre-trade risk (5 limits) | 3.27 µs | 0.31 M/s | 4.8× |
| Order construction + validate | 1.60 µs | 0.63 M/s | 11.4× |

Nothing in this repository has measured a million events per second through
an assembled path, and nothing here claims to. The 24 M/s book figure is one
function applied to one pre-built message.

---

## 4. Reading the shape

Three things in the table are worth an argument rather than a row.

**Feature evaluation dominates the decide path**, at roughly six times the
strategy run it feeds. That is the right shape — the graph is where the work
is, and the strategy is a walk over a small arena — but it means a budget for
"time to a signal" is essentially a budget for the feature graph, and adding
features is the change that will move it.

**Arbitrage is the outlier at 5.8 µs**, an order of magnitude above everything
else, because it is not one operation: it searches the graph, prices a path
against three books, and plans three legs with their ordering rationale. It is
also the only stage here that scales with something a venue controls rather
than something the platform declares.

**Capital admission is nearly free**, which is the point. The bound that stops
a cell overspending while cut off from the centre is a comparison, not a
service call, precisely so that it can be consulted at every use rather than
once on arrival.

---

## 5. What would make these numbers wrong

* **A different machine.** These are single-core figures from whatever ran the
  suite. Reproduce them before quoting them.
* **Contention.** Every measurement is single-threaded. A cell under load
  shares its book between the feed and the decide loop, and nothing here
  measures that.
* **A larger fixture.** The arbitrage graph has three nodes; a real venue set
  has hundreds. The book has four levels; a real one has hundreds. The
  scaling behaviour is not measured, only the point.
* **Allocation.** These run warm, with the fixture already built. First-touch
  costs are not included.

When any of those changes, the honest move is to re-run the suite and rewrite
this document from the new output, not to interpolate.
