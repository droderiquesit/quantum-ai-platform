# 0026 — Telemetry export: bounded OpenTelemetry emission against the Prometheus exposition that exists

**Status:** proposed — the owner decides. This record decides nothing; it
frames one decision the blueprint's observability section requires and no
record has yet named, and it carries a recommendation marked as one.
**Would amend, if accepted in one direction:** ADR 0002 and ADR 0009 (a
crate), or nothing at all (the other two options add no dependency).
**Does not touch:** `.claude/rules/domains/observability.md`, which is the
owner's and is quoted here rather than corrected.

## Context, as verified in the tree at `851c0ed`

### What the architecture of record asks for

- §47: "A live platform graph built from OpenTelemetry spans. Every hop emits
  a span with a stable node_id; the hot path writes to a bounded ring drained
  by a separate thread; graph-builder materialises nodes and edges into
  Spanner; the portal renders it as server-side SVG with click-through"
  (`blueprint.md:4159`). Then a table of signal categories — cognition,
  counterfactual, self-model, valuation, ingestion, strategy engine, netting,
  per-strategy, cycle economics, dispersion, market making, capital, solver,
  wallet, policy freshness (`blueprint.md:4161-4191`) — closing with "Belief
  calibration is the single most important metric" (`:4195`).
- §44.2, bounded state: "Telemetry ring | Fixed byte capacity. Oldest dropped,
  counter alerted" (`blueprint.md:3997`), and "an unbounded collection in
  hot-path code is a review rejection" (`:4001`).
- §45.1: "Cloud Trace, Logging, Managed Prometheus | Observability fed by
  OpenTelemetry from Rust" (`blueprint.md:4039`). Note the third noun: the
  blueprint's metrics backend *is* Managed Prometheus.
- §2.1: managed services are Google Cloud or IBM only; "external
  observability" is excluded (`blueprint.md:141`).
- Rule 74: "Every service emits OpenTelemetry spans with a stable node_id. A
  component that cannot be traced does not ship" (`blueprint.md:4989`).
- Principle 10, "Degrade, do not fail": every dependency loss has a defined
  reduced-capability mode (`blueprint.md:203`); §6.2's degradation order names
  policy freshness and the kill switch as cached with TTL (`:2737-2743`).
- The diagram: "Observability opentelemetry · tracing To Cloud Trace and
  Managed Prometheus" (`ref/index_text.txt`).

The phrase in the brief, "bounded OpenTelemetry emission", is this record's
name for the conjunction of §47's spans, §44.2's ring, and §45.1's
destinations. The blueprint never uses the three words together; each half is
sourced above.

### What the tree emits, and who emits it

**The library.** `backend/crates/libs/qip-observability` is a lib with no I/O:
a `BTreeMap` of series behind one mutex (`metrics.rs:257-276`), a tracer, a
logger, and SLOs. Its own description: "Semantics follow OpenTelemetry
(trace/span ids, span kinds, attribute naming, histogram buckets) so the
exported JSON maps onto an OTLP collector without translation, but the
implementation is in-tree: the collector is a deployment concern, and a
platform that cannot report on itself when the collector is unreachable is
not observable" (`lib.rs:3-7`). It depends on `serde`, `serde_json` and
`qip-core`.

**Metrics.** `Snapshot::to_prometheus` renders text exposition with `# HELP`
and `# TYPE` lines, escaped label values, and cumulative histogram buckets
(`metrics.rs:210-254`). Every published name is a constant in
`metrics::names` (`metrics.rs:455-731`), around ninety of them, "centralised
so dashboards, alerts and the documentation-drift test all read from one
list". The kernel's `Platform` records at the seams (`platform.rs`; the
observability rule says at least sixteen sites and asks for a recount before
quoting); the edge cell records through `CellMetrics` into a registry it is
given, never one it reaches for; each edge site is proven by a test in
`qip-edge/tests/telemetry.rs:233-540` (eight tests, halt gauge through
reconciliation break).

**Exposition.** `qip-api` serves `/metrics` as text (`routes.rs:44,564`) and,
separately, `/system/metrics` as JSON — "what has this process done since it
started" against "how big the book is now", deliberately not merged
(`routes.rs:912-955`). `qip-edge-node` serves `/metrics` on its health port
with `text/plain; version=0.0.4` (`qip-edge-node/src/telemetry.rs:55-58`).
`qip-fastbrain` and `qip-deepbrain` serve from the same registry the cycle
writes to (observability rule; `NOT-SCRAPED.md:25-27`).

**Traces.** The tracer exists and is OpenTelemetry-shaped: W3C-length ids
(`trace.rs:1-4`, test `span_ids_have_the_w3c_lengths`), deterministic ids so a
replay reproduces the trace (`trace.rs:174-185`), a **bounded ring of
10,000 finished spans, oldest dropped** (`trace.rs:122-123,187-193`), and
`Tracer::export` producing `resourceSpans`/`scopeSpans` JSON
(`trace.rs:221-236`; test `trace_export_has_the_otlp_shape`,
`tests/telemetry.rs:201`). **No production code starts a span.** A search for
`.tracer` outside the library finds nothing across `backend/crates`. Rule 74
is therefore unmet not for want of an exporter but for want of a producer.

**Logs.** Structured records carrying trace and correlation ids
(`logs.rs:1-6`), rendered by `to_line`; nothing ships them anywhere but a
terminal.

**Correlation.** `qip_core::lineage::Lineage` carries `correlation_id`,
`causation_id` and an optional `trace_id` on every event (`lineage.rs:62-70`);
`TraceId` is documented as "exported to OpenTelemetry" (`lineage.rs:41`).
Cross-plane correlation exists today **in the event log**, keyed by
correlation id (`docs/ops/observability/README.md:9-14`), and nowhere in the
metrics — correctly, since an id is an unbounded label.

**OTLP.** Nothing in `infrastructure/` names OTLP, OpenTelemetry or a
collector (grep, no match). Nothing in `backend/` names OTLP except the
library's own shape test and doc comments. No OpenTelemetry crate is in the
lockfile; `check-dependencies.sh:19-35` permits eleven packages.

### What "bounded" is already enforced, and where it is not

Enforced:

- **Label cardinality** is bounded by construction: `cell` and `region` are
  one value per process, `venue` is the configured list, `gate` is a set of
  source literals, and `source`, `capability`, `kind`, `outcome`, `state` and
  `direction` are enums; nothing is labelled by instrument, strategy or order
  id (observability rule, "Every label is bounded"). Label values from
  configuration are escaped at the exposition so a hostile cell id cannot
  forge a line (`metrics.rs:424-446`; test
  `a_label_value_from_configuration_cannot_forge_or_break_an_exposition_line`).
- **Histogram buckets** are fixed and explicit (`metrics.rs:29-58`).
- **The span ring** is capped at 10,000, oldest dropped (`trace.rs:189-191`).
- **Capability freshness is published only for the three capabilities the cell
  measures**; `ingestion` and `counterfactual_scoring` are deliberately absent
  rather than a permanent `unavailable` (observability rule).

Not enforced, and worth the owner's eye against §44.2:

- The **metrics registry has no byte cap**. It is bounded by label discipline,
  not by type; a new recording site with a free-text label would grow it
  without a test refusing. The blueprint asks for "fixed byte capacity" and
  says "in Rust the capacity is visible in the type signature". Today it is
  visible in a rule file.
- The span ring **drops silently** — there is no "counter alerted" for the
  drop (`trace.rs:187-193`).
- Nothing drains either on a separate thread, because nothing exports.

### What is collected, and what is not

`infrastructure/terraform/modules/observability/main.tf` declares seven alert
policies in PromQL — kill switch, live fill, persistent breach, permission
violation, edge halted, edge reconciliation break, central reconciliation
break (`main.tf:23,59,97,131,176,213,253`) — every one gated on
`workload_metrics_exist` because Cloud Monitoring refuses a policy naming a
descriptor it has never ingested (`main.tf:7-14`).

`NOT-SCRAPED.md` states the collector position exactly:

- The execution node's Ops Agent has a Prometheus receiver on
  `localhost:<health_port>/metrics` declared in its startup script, carrying
  `qip_edge_*` to Cloud Monitoring as `prometheus.googleapis.com/...`
  descriptors. **No node exists**; `execution_nodes` is empty in every
  environment (`NOT-SCRAPED.md:9-21`).
- The Cloud Run services have no collector. The candidate is Google's
  managed-Prometheus sidecar, which must be vendored by digest through
  `infrastructure/egress/vendored-images.txt` and admitted by Binary
  Authorization before any revision carrying it can deploy. **Nobody has
  pinned that digest.** A `metrics_sidecar` input on `modules/cloudrun` is
  named as the shape of the change and does not exist in any `.tf` at
  `851c0ed` — the only mention is `NOT-SCRAPED.md:48-50` (`NOT-SCRAPED.md:23-44`).

So the state is: emitted, scrapable, never scraped, on any runtime, ever. The
observability rule's instruction — "Do not describe this platform as
observable" — stands.

### The signals the blueprint names, and which exist by name

| Blueprint signal (§47, §6.2) | Exists by name in `metrics::names` | Where recorded | Missing |
|---|---|---|---|
| Policy freshness — age of the shipped items per region (`:4191`) | `qip_edge_policy_sequence` (`:614`), `qip_edge_capability_freshness{capability}` (`:608`) | Cell, per pass | The *age* as a duration; what exists is the applied sequence and a freshness class per capability. Nothing at the centre publishes what it believes it shipped, so the correlation the rule file describes is one-sided |
| Belief calibration — the single most important metric (`:4163,4195`) | `qip_belief_brier_score` (`:679`), `qip_belief_confidence_adjustment` (`:682`), `qip_belief_evaluations` (`:685`), `qip_theses_evaluated_total` (`:688`) | Kernel LEARN, since `04738ee` | Calibration by confidence bucket ("seventy percent happens seventy percent") is one Brier number, not a curve |
| Veto and counterfactual — veto profitability by rule, venue, regime; feasibility rejection distribution (`:4165`) | `qip_risk_rejections_total` (`:492`), `qip_orders_refused_total` (`:524`), `qip_edge_refusals_total{gate}` (`:603`), `qip_counterfactuals_scored_total`, `_regrets_total`, `_deferred_total`, `_unscored_total` (`:692-702`) | Kernel and cell | Regret *by rule and venue* is not a label set the tree carries; regime is not a label at all. The feasibility gate's eight literals are the `gate` label and do give the rejection distribution at the cell |
| Reconciliation — deltas and breaks (`:4189`) | `qip_edge_reconciliation_breaks_total` (`:623`), `qip_central_reconciliation_breaks_total{direction}` (`:649`), `qip_central_cell_halts_total` (`:655`) | Cell finds; centre acts | The delta magnitude; only counts exist |
| Degradation — reduced-capability modes (`:203`, `:2737-2743`) | `qip_edge_sizing_multiplier` (`:611`), `qip_edge_halted{source}` (`:602`), `qip_edge_mesh_circuit{state}` (`:633`), `qip_quantum_fallbacks_total` (`:488`), `qip_universe_not_decision_grade` (`:731`), `qip_stage_problems_total` (`:514`) | Cell and kernel | A single "mode" gauge per plane naming which §6.2 row the process is in |
| Cross-plane correlation ids | None as metrics — correctly | `Lineage` on every event (`lineage.rs:62-70`) | Spans. The event log answers "what happened in cycle X"; nothing answers "how long did hop Y take inside it" |
| Self-model — coverage gaps, estimator error against bound (`:4167`) | None | — | No self-model exists in the tree (traceability, Plane 2) |
| Spans with a stable `node_id` (`:4159`, rule 74) | The tracer, unused | — | A producer; a `node_id` scheme; a drain |

The table is the honest answer to "what does bounded OpenTelemetry emission
look like here": most of the *metrics* the blueprint names already exist by
name and are bounded; none of the *spans* do, and the correlation the
blueprint wants from spans is today available only from the event log.

## Options

### Option (a) — keep Prometheus exposition; the collector translates; no dependency

The processes keep serving text at `/metrics`. On the node, the Ops Agent's
Prometheus receiver (declared) carries series to Managed Prometheus. On Cloud
Run, the managed-Prometheus sidecar (to be vendored by digest, admitted by
Binary Authorization, attached through a `metrics_sidecar` input shaped like
`egress_sidecar`) scrapes loopback. Translation to Google's descriptors is the
collector's job. Alert policies flip on per environment once a descriptor is
observed.

*What stays bounded:* everything that is bounded today; the exposition is a
snapshot of a registry whose cardinality is fixed by enum and configuration.
The collector adds no label the process did not emit.

*What the blueprint's signals look like:* exactly the table above — metrics,
no spans. Managed Prometheus is the backend §45.1 names, so this option is
aligned for metrics and silent on Cloud Trace.

*What closes it:* a `prometheus.googleapis.com/qip_edge_halted/gauge` or
`qip_cycles_total/counter` descriptor visible in a project's metric explorer;
then `workload_metrics_exist = true` in that environment's tfvars; then the
seven policies exist. `NOT-SCRAPED.md:46-54` already lists this.

*What it costs:* rule 74 stays unmet. §47's platform graph has no source. The
sidecar is a vendored third-party image in the trust boundary, reviewed by
digest like Envoy. Nothing here can be proven from this environment — no
Terraform binary, no project reachable (ADR 0024).

*Dependency:* none in Rust. One more vendored image.

### Option (b) — hand-rolled OTLP/JSON over HTTP, `serde_json` only; in-tree protocol code

Keep (a) for metrics. For spans: a producer records into the ring that exists;
a drain thread in each composition root takes the ring's contents on an
interval and POSTs OTLP/JSON to a collector on loopback — an OpenTelemetry
collector sidecar on Cloud Run, or the Ops Agent's OTLP receiver on the node —
with an explicit timeout, blocking, on the pattern every other outbound call
here follows. `Tracer::export` already produces the top-level shape
(`trace.rs:221-236`).

*What stays bounded:* the ring's capacity (add the drop counter §44.2 asks
for); the drain is one thread that never touches the order path; a collector
that is down means spans are dropped and counted, never buffered without
limit — principle 10 applied to telemetry itself.

*What the blueprint's signals look like:* every metric as in (a), plus spans
per cycle stage at the centre and per work pass at the cell, each carrying
`service.name` (already set, `trace.rs:156`), a `node_id` attribute, and the
event `correlation_id` as an attribute so a span joins the log by the same
key the log already uses. That is the cross-plane correlation the blueprint
wants, built on the id the tree already has.

*Where the code lives, and why that is not a layering violation:*
`qip-observability` stays a lib with no I/O — it owns the ring, the encoder
and the drop counter. The drain thread and the HTTP POST are composition-root
code in `apps/`, which is where every other outbound socket in this platform
lives. Services keep recording into the `Telemetry` they are given. No lib
performs I/O; no service reads a collector address.

*What closes it:* a span with a known `node_id` visible in Cloud Trace after
a POST from a deployed process; the `qip_observability_spans_dropped_total`
counter (or whatever it is named) present in the exposition; and a test that
drives a cycle and asserts the ring holds a span per stage, mutation-verified
by removing one recording site.

*What it costs:* protocol code in-tree. OTLP/JSON is a stable, published
mapping of a protobuf schema, but it has the quirks such mappings have — ids
as hex, 64-bit integers as strings, timestamps in nanoseconds as strings, the
attribute-value envelope — and a mistake is refused by the collector rather
than silently misread, which is the loud failure ADR 0012 says does not earn
a dependency. It also costs a collector: on Cloud Run, an OpenTelemetry
collector image vendored by digest, the same shape of work as (a)'s sidecar
and possibly the same container if Google's sidecar accepts OTLP. That
question is answerable only by reading the sidecar's documentation and
pinning a digest, neither of which this record does.

*Dependency:* none. The encoder is `serde_json`.

### Option (c) — an OpenTelemetry crate

The published crates and their OTLP exporter. Stated as ecosystem claims for
the owner to verify against a lockfile, not as facts about this tree: the
OTLP exporter ships over an HTTP or gRPC client that brings an async runtime;
protobuf codegen crates come with it; the closure is large.

*What stays bounded:* the SDK's batch processor has its own queue with its
own bound and its own drop behaviour, configured rather than typed. Label
discipline stays ours.

*What the signals look like:* as (b), with instrumentation macros rather than
explicit calls.

*What it costs:* the async runtime this platform has refused three times
(ADR 0001 by implication, ADR 0012 explicitly, the boundaries rule in terms);
a transitive tree in every composition root; `check-dependencies.sh` moving
to per-tier; and the loud-failure argument from ADR 0012 cutting against it —
an exporter that fails is visible in the collector, so condition 1 is not
met. It would reach no decision-core crate, so ADR 0009's tiering holds, but
the composition roots are the processes that serve posture and route orders
to the simulator, and the boundaries rule was written about exactly them.

*Dependency:* a reversal of ADR 0012's async-runtime refusal and an amendment
to ADR 0002/0009 with a `PERMITTED` list several dozen lines longer, each
with a reason.

## Recommendation — marked as a recommendation, not a decision

**Option (a) for metrics, now and as the standing answer; Option (b) for
spans, sequenced behind a producer; (c) rejected.**

Why, in the order the reasons weigh:

- **The blueprint's own metrics backend is Managed Prometheus** (`:4039`). The
  exposition that exists is the format that backend ingests. Translating it to
  OTLP so a collector can translate it back is work with no signal in it.
- **Every gap that matters is a producer gap, not an exporter gap.** No span
  is started anywhere; the calibration curve, the regret-by-rule labels, the
  policy age and the per-plane degradation mode are recording sites nobody
  has written. An exporter chosen before those exist exports nothing. The
  first slice is therefore `Platform::run_cycle` and `Cell::work` each
  opening a span per stage with a `node_id` and the correlation id, into the
  ring, with a drop counter — and that slice needs no export decision at all.
- **(b) is the blueprint's mechanism, sentence for sentence** — "hot path
  writes to a bounded ring drained by a separate thread" — and it is buildable
  under the rules as they stand: blocking I/O with a timeout, in a
  composition root, from a lib that does no I/O.
- **(c) reopens a settled decision for a loudly-failing convenience**, and the
  repository's own three-part test says no. If the owner nonetheless wants
  the crate, the record that admits it is the one that also admits the async
  runtime, and it should be written as that.
- **Nothing here changes what a person can check.** The evidence for (a) is
  a descriptor in a metric explorer; for (b) a span in Cloud Trace with a
  `node_id` that appears in this repository's source. Both are things a
  reviewer can see, which the product rule asks of every proposal.

The honest limit: none of it can be observed from this environment. ADR 0024
records no Terraform binary, no project reachable, nothing applied. This
record can decide a shape; only an apply and a scrape can decide whether the
shape works.

## What it costs

Each option's cost is stated beside it above; this is the sum for the
recommended pair. Option (a) for metrics costs one more vendored third-party
image inside the trust boundary — the managed-Prometheus sidecar, reviewed and
pinned by digest like Envoy — and leaves rule 74 unmet and §47's platform
graph without a source until a descriptor is actually observed, which cannot
be proven from this environment. Option (b) for spans costs protocol code in
the tree: an OTLP/JSON encoder whose mistakes are refused loudly by the
collector rather than silently misread, a drain thread in every composition
root, a drop counter, and a second collector question (whether Google's
sidecar accepts OTLP) that only a pinned digest and its documentation can
answer. Neither costs a dependency. Option (c) would cost the async runtime
this platform has refused three times and a `PERMITTED` list several dozen
lines longer, in the processes that serve posture and route orders to the
simulator — which is why it is rejected.

## What would make this wrong

- **Google's managed-Prometheus sidecar cannot be admitted** — a digest that
  will not attest, or a Cloud Run constraint on a second scraping container
  beside the Envoy sidecar. Then (a) has no collector on Cloud Run and the
  execution node is the only scraped process; (b)'s push model becomes the
  only path off a Cloud Run service, and it would then carry metrics too,
  which is a larger in-tree encoder than this record priced.
- **The Ops Agent or the sidecar does not accept OTLP/JSON over HTTP.** Then
  (b) needs OTLP/protobuf, which is a binary encoding this repository would
  have to write and test against published vectors — still no dependency, but
  a materially larger piece of in-tree protocol code, and the point at which
  ADR 0012's condition 2 (specialist) starts to apply and (c) starts to earn
  its cost.
- **The span ring is found on the order path.** If a drain thread contends
  with `Cell::work` for the ring's lock, the bound is honoured and the latency
  budget is not, and the blueprint's "drained by a separate thread" needs a
  lock-free structure this tree does not have.
- **Metrics turn out to need a byte cap in the type**, not in a rule file. A
  recording site landing with an unbounded label would prove the discipline
  insufficient, and the fix is a typed registry capacity with a refusal
  counter — a change to `qip-observability` this record does not make.
- **The blueprint's platform graph (§47) becomes load-bearing** — an operator
  actually needs click-through from a node to its spans. Then graph-builder
  and its store are the next decision, and Spanner re-enters through the door
  ADR 0011 left for analytics.

## What this does not decide

- It does not vendor, pin or attach any sidecar image, and it does not add
  `metrics_sidecar` to `modules/cloudrun`. That is infrastructure work with
  its own evidence requirement (a plan a person reads).
- It does not flip `workload_metrics_exist` anywhere. The rule is unchanged:
  evidence a pod actually scraped, first.
- It does not add an alert policy, and in particular does not add one naming
  a span or a descriptor nothing has ingested.
- It does not define the `node_id` scheme. The obvious candidate — the
  service name plus the cell id for the edge, both already labels — is noted,
  not chosen.
- It does not decide the graph-builder, its store, or the portal's SVG
  rendering of it.
- It does not write a recording site. The producer slice named above is
  handed to an implementer with the mutation-verification rule attached.
- It does not touch the paper-trading boundary; nothing in telemetry export
  reaches a venue, a credential or a ceiling, and `qip_live_fills_total`
  stays the alarm it is.

## Dependency-direction argument

Under (a), no Rust code changes. Under (b), `qip-observability` remains a lib
that performs no I/O — ring, encoder, counters — and the drain thread with its
loopback POST lives in `qip-api`, `qip-fastbrain`, `qip-deepbrain` and
`qip-edge-node`, which already own every other socket. Services record into a
`Telemetry` handed to them and gain no edge. No lib depends on a service, no
service on the runtime, nothing on an app. Under (c), the crate would be
declared by the composition roots and, if instrumentation macros were used
inside services, by services too — which is the widening ADR 0009 warns
about, and one more reason (c) is not recommended.
