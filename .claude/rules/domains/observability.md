# Domain: observability and SRE

**Scope** — `backend/crates/libs/qip-observability/**`, `docs/ops/observability/**`, and the
health surfaces in `backend/crates/apps/**`

## The state of this domain

**Both planes emit and both are scrapable. What is still missing is proof
of ingestion, and the edge plane's recording sites in `Cell::work` reach no
deployed process until a node is deployed — the binary runs passes since
`6340610`, and none is deployed.** Read all of this before
writing anything about the domain: two earlier versions of this file were
each false in one direction — one said nothing wrote to `Telemetry`, the next
said the edge plane could not emit — and agents who believed either were told
a closed gap was still open.

`qip-kernel`'s `Platform` records in `platform.rs` — **do not quote a figure
from this file; run the command**:
`grep -c 'metrics\.\(count\|gauge\|increment\|observe[a-z_]*\)(' backend/crates/runtime/qip-kernel/src/platform.rs`.
This paragraph said "at least sixteen sites as of `6fb5fed`" until 2026-09-06.
That was a floor, not a measurement, and it was read as a measurement; it had
drifted to double without ever becoming false, which is the failure mode a
floor has and a count does not — a number nobody could catch being wrong. The
figure is deliberately not restated here. On 2026-09-06 the same command
printed `32` and then, about twenty minutes later on the same checkout,
`34`, because a parallel lane was mid-rewrite of `platform.rs`. A number
written down in this file is a number measured on somebody else's
half-finished edit. What is recorded matters more than how many times: cycles and per-stage runs,
durations and problems; the kill-switch gauge; limit breaches; permission
denials; orders submitted, refused, filled and the live-fill alarm. All three central binaries construct the `Telemetry` the
cycle writes to and serve its snapshot: `qip-api` from `routes.rs` (`scrape`),
`qip-fastbrain` and `qip-deepbrain` from the registry handle their health
servers hold, taken from the same `Telemetry` before it moves into the
`Platform`. `/metrics` is empty only until the first cycle records something,
which is the honest answer rather than a gap. `qip-market-ingestion` also
records, but `IngestionService` is constructed only in its own tests, so those
three sites reach no deployed process.

**The edge plane emits through `qip_edge::CellMetrics` and is scraped at
`/metrics` on `qip-edge-node`'s health port.** `qip-edge` depends on
`qip-observability` — a library holding a `BTreeMap` behind a mutex, no I/O —
and a `Cell` records into a registry it is *given* by `Cell::with_metrics`,
never one it reached for. `qip-edge-node` constructs one `Telemetry`, takes
the registry handle first, hands it to the cell and to `MeshSeries`, and
serves the same handle's snapshot as Prometheus exposition; every other path
still answers the JSON health body. The series and what each is keyed on:

- `qip_edge_halted{source}` — a gauge per halt discipline (`kill_switch`,
  `policy`, and since `ff86473` `polled`, §46.2's second wire — the flag
  `qip-edge-node` polls on its own filesystem; `qip-edge/src/telemetry.rs:294-314`
  writes the three `source` arms and `:191-194` describes the gauge — this
  bullet cited `:172-193` until 2026-09-06, which by then landed on the
  descriptor block and not on the writer. Locate both with
  `grep -n 'fn halt(\|names::EDGE_HALTED' backend/crates/edge/qip-edge/src/telemetry.rs`),
  written wherever any halt can change and at wiring time, so a cell halted
  before its first pass still reports halted.
- `qip_edge_capability_freshness{capability}` and `qip_edge_sizing_multiplier`
  — the §6.2 table as the pass actually sized against it, recorded per pass.
  Only the three policy-fed capabilities are published; `ingestion` and
  `counterfactual_scoring` are deliberately absent because the cell never
  measures them and a permanent `unavailable` would be a number nobody
  computed.
- `qip_edge_policy_sequence` — the payload the cell has *applied*, for
  correlation against what the centre believes it published.
- `qip_edge_region_share_bound` and `qip_edge_region_share_applied_total{outcome}`
  — the ADR 0039 share as the cell's ledger actually took it, recorded at the
  seam where a share is applied or re-derived (`qip-edge/src/cell.rs`,
  `apply_region_share` and `rederive_region_share`). The gauge carries the
  bound the ledger returned, not the share that was offered, because the
  operator's ceiling may have capped it. `outcome` is
  `qip_edge::telemetry::RegionShareOutcome`: `applied`, `rederived`,
  `refused_lower_sequence`, `withheld`. Four rather than one because a bound
  that did not move reads identically whether the centre narrowed the cell,
  refused a replay, or has stopped saying anything about capital — and the
  last of those is an outage wearing a plan's clothes. `withheld` is recorded
  only by a cell that holds a table; a refusal the cell cannot attribute to
  the sequence is journaled under the `region_share` gate and deliberately
  left off the series rather than filed under a cause nobody established.
- `qip_edge_work_passes_total`, `qip_edge_fills_confirmed_total{venue}` (a
  fill the venue reported, and nothing else — `cb79b46`),
  `qip_edge_orders_expired_total{venue}` (a rested order withdrawn when its
  time to live elapsed — `383d4e7`), `qip_edge_refusals_total{gate}`,
  `qip_edge_signals_raised_total{kind}`, `qip_edge_orders_placed_total{venue}`,
  `qip_edge_intents_cancelled_total`, `qip_edge_internal_crosses_total{venue}`,
  `qip_edge_netting_ratio` (histogram), `qip_edge_reconciliation_breaks_total`.
- From the node: `qip_edge_mesh_{deltas,grants,policy_frames}_total{outcome}`
  as deltas of the link's cumulative counters, and
  `qip_edge_mesh_circuit{state}`.

Every label is bounded by something fixed at deployment or by an enum or a
source-file literal: `cell` and `region` are one value per process, `venue`
is the configured venue list, `gate` is the set of string literals
`Cell::refuse` is called with, and `source`, `capability`, `kind`, `outcome`
and `state` are enums. Nothing is labelled by instrument, strategy or order
id. Each recording site is proven by a test in
`backend/crates/edge/qip-edge/tests/telemetry.rs` that drives the cell through
the event and asserts the series moved, and each was mutation-verified.

Two honest limits on the edge half. First, `qip-edge-node` runs `Cell::work`
only when `QIP_VENUE_FEED=simulated` (`6340610`; `run_pass` at
`qip-edge-node/src/pass.rs:106`, called from `main.rs:779` — recount with
`grep -n 'fn run_pass' backend/crates/apps/qip-edge-node/src/pass.rs` and
`grep -n 'run_pass(' backend/crates/apps/qip-edge-node/src/main.rs`; these
read `:118` and `:586` until 2026-09-06; the execution
node's template writes the line at `startup.sh.tftpl:174`
(`grep -n QIP_VENUE_FEED infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl`,
verified 2026-09-06), and any other
value stops the process naming ADR 0003), so the pass-time series —
freshness, refusals, signals, orders, netting, crosses, and
`qip_edge_work_passes_total`, `qip_edge_fills_confirmed_total` and
`qip_edge_orders_expired_total` — reach a process that runs passes **only in
test** (`qip-edge-node/tests/pass.rs::a_node_with_the_simulated_feed_runs_a_pass_and_the_pass_time_series_move`).
Nothing is deployed, because `execution_nodes = {}` in every environment.
An earlier version of this paragraph said the binary never called
`Cell::work`; that was true until `6340610` and is not now. Second, the
edge-node health server is single-threaded, so
a scrape's exposition is rendered on the thread that flushes the journal — the
same thread that already renders the JSON body, and not the order path.

**A central-plane reconciliation break is now recorded too.**
`CentralPlane::record_halt` in `central/plane.rs`, reached from `ingest` and
so from `Platform::ingest_cell_report` — **located by symbol, not by line, and
that is deliberate**: this read `plane.rs:1398` until 2026-09-06, by which
time the function had moved some two hundred lines and the citation pointed
at unrelated code. Find it with
`grep -n 'fn record_halt\|self.record_halt(' backend/crates/runtime/qip-kernel/src/central/plane.rs`.
It counts
`qip_central_reconciliation_breaks_total` by the direction of the gap —
`cell_over_venue`, `venue_over_cell`, `detail_only`, and since `3c2b789`
`unsent_fill`, a fill a cell reported on an order the centre never saw sent
or beyond the quantity it saw sent — and `qip_central_cell_halts_total` by
cause, on the outcome rather than the report, so a refused report charts no
halt. The `unsent_fill` direction is the centre's own finding rather than the
cell's, raised where `settle` pushes a `BreakOrigin::UnsentFill` and merged
with the report's breaks before the halt, where `ingest` builds its `breaks`
vector (`grep -n 'let breaks: Vec<ReconciliationBreak>\|BreakOrigin::UnsentFill' backend/crates/runtime/qip-kernel/src/central/plane.rs`;
this cited `plane.rs:1066` until 2026-09-06 and that line was in a different
function by then). The
`qip_central_` prefix keeps it distinct from the edge's own break counter,
which records what the cell found rather than what the centre acted on.
Beside it, since `9e45dc0`, `qip_central_orders_sent_total`
(the `CENTRAL_ORDERS_SENT` constant in `qip-observability/src/metrics.rs`,
recorded in `CentralPlane::settle`; this said `metrics.rs:711` and
`plane.rs:1187` until 2026-09-06 and both were wrong by well over a hundred
lines. Locate both with
`grep -n 'CENTRAL_ORDERS_SENT\|CENTRAL_FILLS_ATTRIBUTED' backend/crates/libs/qip-observability/src/metrics.rs backend/crates/runtime/qip-kernel/src/central/plane.rs`)
counts
the orders a cell reported sent and `qip_central_fills_attributed_total`
counts the fill shares the centre booked, so that what was sent and what
filled are two series — for one slice the centre billed every sent order as a
fill, and there was no series in which the two claims could disagree. Two
facts that once had no production caller now have one, in the LEARN stage:
`Platform::learn_from`, which produces the belief calibration, is called from
`calibrate_resolved`, which `stage_learn` calls (`04738ee`), and
`Platform::evaluate_alternatives`, which scores counterfactuals, is called
from `score_declined`, which `stage_learn` also calls (`b9e2242`). **The four
line numbers this passage used to give — `:4086`, `:3965`, `:5107`, `:3982` —
were all wrong and are removed rather than replaced.** They came from a
`platform.rs` roughly half the length of the present one and every one of them
landed in unrelated code; the passage already carried a recount command and
the command was not run, which is the exact way a citation rots here — it is
quoted forward because it was quoted forward. They are not replaced with
fresh numbers because fresh numbers do not survive either: on 2026-09-06 the
four call sites moved by more than a hundred lines each inside a single
working session, while a parallel lane was editing the file. Name the symbol,
run the command. Recount with
`grep -n "learn_from\|evaluate_alternatives" backend/crates/runtime/qip-kernel/src/platform.rs`
before quoting either line.

`workload_metrics_exist` remains `false` everywhere — the default in
`infrastructure/terraform/variables.tf` and in the observability module, and
commented out in `environments/dev/terraform.tfvars` — and flipping it still
requires evidence something actually scraped. All **nine** alert policies in
`modules/observability/main.tf` are gated on it and name descriptors the
binaries do record: seven central-plane policies, `edge_halted` and
`edge_reconciliation_break`. Nine, and exactly two of them query a
`qip_edge_*` series — recount with
`grep -c '^resource "google_monitoring_alert_policy"' …/main.tf` and
`grep -c 'query *= *"[^"]*qip_edge' …/main.tf` before quoting either number,
and check the third figure too, because it is the one that carries the
meaning: `grep -c 'count *=.*workload_metrics_exist' …/main.tf` must equal
the first. Declared 9, gated 9, edge 2 on 2026-09-06.

It was seven until `599daaa` added `risk_figure_unevaluated` (on
`qip_risk_figures_unevaluated`) and `sign_off_withheld_on_liquidity` (on
`qip_proposals_unsigned_total{control="liquidity-read"}`), so a book whose
liquidity cannot be read is no longer charted for nobody. Both fire on a
control **working** rather than failing: the platform has stopped trading that
book, not started trading it badly, and an operator who reads either as a
fault will look for the wrong problem. Their documentation says so in its
first sentence.

This paragraph has now been wrong twice about this number, in both
directions, which is why the recount commands are here rather than the
figures alone. `NOT-SCRAPED.md` said "three edge policies" for a while, which
made the two groups sum to eight; this file then said seven for a day after
the count reached nine. **Recount before you quote.** Verified 2026-09-05
that each of the then-seven names has a
production recording site rather than only a registered constant: the four
central ones and `qip_central_reconciliation_breaks_total` in
`runtime/qip-kernel/src/{platform.rs,central/plane.rs}`, and `EDGE_HALTED`
and `EDGE_RECONCILIATION_BREAKS` in `edge/qip-edge/src/telemetry.rs` outside
`tests/`. A registered constant nothing calls would satisfy the acceptance
test and still page nobody, so check the caller, not the name. Since
`cd16f79` the `edge_halted` policy's documentation names the `polled` source
beside `kill_switch` and `policy` (`main.tf:206`, inside the `documentation`
block that opens at `:201`; this said `:200` until 2026-09-06, which is the
blank line above the block), so an operator paged on
the third source is not reading text that says it does not exist. What
collects
is the runtime's business (ADR 0024) and `modules/observability/NOT-SCRAPED.md`
is the record: the `PodMonitoring` that once selected the two brains left
with the cluster, and nothing scrapes a Cloud Run service. The execution
node's startup script declares an Ops Agent Prometheus receiver on the health
port (`startup.sh.tftpl:209-231`), so `qip-edge-node` is scraped once a node
exists; `execution_nodes = {}` in every environment
(`grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars`),
so none does. What is missing is proof of ingestion, not emission.

The two halves of the collection gap are in **different states**, and an
earlier version of this paragraph flattened them into one. The node's
receiver is merely waiting on a node. The Cloud Run half is **refused, not
pending**: the managed-Prometheus sidecar is declared in `modules/cloudrun`
and rendered only under `metrics_collector_image_digest`, which is null
everywhere, because the only version Google publishes fails the platform's
own Trivy gate. `vendor.yml` run 11 failed `cloud-run-gmp-sidecar` 1.9.2 on
`CVE-2026-56854`, an unfixed CRITICAL in `golang.org/x/crypto` v0.54.0.
Re-checked 2026-09-05 and the position has degraded rather than improved: the
registry's tag list still ends at 1.9.2 with no 1.9.3 or 1.10.0, the
`docker-content-digest` for 1.9.2 is still
`sha256:ff1fc688...df522fd1` so the scanned bytes are unchanged, and two
further `x/crypto/ssh` advisories fixed in 0.56.0 landed on 2026-09-02
(`GO-2026-6354`/`CVE-2026-78662`, `GO-2026-6355`/`CVE-2026-56855`), putting
the image three advisories behind. The commands are in `NOT-SCRAPED.md`.
The blocker is upstream and no change in this repository clears it; do not
clear it with a scanner exception.

One correction of fact this file used to get wrong by inheritance:
`NOT-SCRAPED.md` said "nothing has been applied", and `dev` has been —
`module.observability` is instantiated unconditionally at
`terraform/main.tf:461` (`grep -n 'module "observability"' infrastructure/terraform/main.tf`;
this read `:400` until 2026-09-06), so it was in `infra.yml`'s `up`. With the
gate false, `count = 0` on all **seven policies that existed at that apply** —
there are nine now, and the sentence is kept in the past tense on purpose so
that a reader does not take it for a present count — so that apply created no
policy. The gate has run, not merely been declared. This changes nothing about
ingestion.

Do not describe this platform as observable. That still holds, on today's
evidence: nothing has been shown to scrape any process, no Cloud Run service
has a collector at all, and every alert policy is absent until
`workload_metrics_exist` is flipped, so a reconciliation break on either
plane is charted and still pages no one. Closing the remainder is tracked
work.

## Approved

- Metrics recorded at the seam where the fact becomes known, not inferred later.
- Health endpoints reporting real readiness — storage proven writable, ports
  bound — rather than process liveness.

## Prohibited

- Naming a metric in an alert policy that nothing emits. Cloud Monitoring
  refuses a policy naming a descriptor it has never ingested, and a policy
  stored but never evaluated reads in the console as a project being watched,
  which is worse than the gap it replaces.
- Logging a token, key, or account identifier.
- A health check that reports ready before its dependencies are proven.

## Required evidence

The emitting code plus a test asserting the metric is recorded. Flipping
`workload_metrics_exist = true` requires evidence a pod actually scraped.
