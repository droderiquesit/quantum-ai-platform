# Completing the four gates, and wiring telemetry to reach them

**Status at the time of writing:** 0 of 4 blueprint phase-exit gates passed
(`docs/architecture/algorik-blueprint-traceability.md:37-47`). 0 of 7 planes
ALIGNED. The workspace is green — 322 binaries, 3,835 tests, 0 failed — and
that is the point worth internalising before reading the rest: **no gate is
blocked on code quality.** Three are blocked on the platform never having
seen real data, and the fourth is refused on purpose.

This plan is written against the gates rather than against the backlog,
because the backlog has been the organising unit for five waves and the gates
have not moved in any of them. A gate is not a task that can be closed by
merging something. It is a question the platform must answer with evidence,
and for three of the four the evidence does not exist yet at any level of
engineering effort.

## The honest ceiling

**Three, not four.** The Phase 3 gate asks whether the platform survives
contact with a live venue. Paper trading is absolute (ADR 0003, ADR 0021),
three independent layers enforce it, and none of them may be weakened. That
gate is `CANNOT PASS` by design and should be reported as such for ever
rather than carried as outstanding work. Track D below says what would have
to change, and that it is a governance decision nobody has asked for.

So the target is 3 of 4, and the ordering is forced: Phase 2 first, because
Phases 4–19 sit behind it and the blueprint's own §51.1 says of it — "the
most important gate in the document. If no: Stop."

---

## Track 0 — Telemetry: make the platform observable before it is judged

This track is first not because it is a gate but because **no gate can be
argued without it.** Each of the three reachable gates is a claim about
behaviour over time — a family surviving a holdout, a Brier score beating an
implied probability, an allocation beating a baseline out of sample. Today a
reconciliation break on either plane is recorded, charted, and pages nobody,
and a seven-day streaming run would produce no durable record that it
happened. Running the gates without this means asserting results rather than
evidencing them, which is the one thing this repository refuses.

### 0.0 The situation, stated precisely

- **The emitters work.** `qip-kernel`'s `Platform` has 25 recording sites
  (recounted at HEAD:
  `grep -c 'metrics\.\(count\|gauge\|increment\|observe[a-z_]*\)(' backend/crates/runtime/qip-kernel/src/platform.rs`
  → 25). The edge plane emits through `qip_edge::CellMetrics`. Both planes
  expose Prometheus exposition at `/metrics`.
- **The drain exists and is inert.** All three central roots carry an
  OTLP/JSON drain thread. No environment sets `QIP_OPENOBSERVE_URL`, so no
  thread starts anywhere. `manifest_wiring.rs`'s `READ_BUT_NOT_SET` records
  that deliberately.
- **OpenObserve is now deployed and serving** at its Cloud Run URL, anonymous
  on the public internet under ADR 0030, with its own login enforced (the API
  answers 401 unauthenticated). Its database is empty because nothing sends
  to it.
- **Nothing scrapes anything.** All seven alert policies are gated on
  `workload_metrics_exist`, unset in every environment;
  `metrics_collector_image_digest` is likewise unset.

There are therefore *two independent paths* and they are not alternatives —
they answer different questions:

| Path | Carries | Answers |
|---|---|---|
| **Push** — the OTLP drain in each root | that process's own metric snapshot | "what did this process observe" |
| **Pull** — a Prometheus collector scraping `/metrics` | the same series, plus the edge node's | "is this process alive and what does an operator page on" |

The push path is nearly complete and blocked on one structural fact. The pull
path is blocked on a vendored, attested collector image nobody has pinned.
**Do the push path first**: it needs no new image, and it is what a gate
argument actually reads from.

### 0.1 The blocker nobody has written down

`qip_transport::Url::parse` **refuses every scheme but plaintext `http`, by
name.** This is deliberate: the HTTP client speaks plaintext HTTP/1.1 and
expects the egress proxy in front of it (ADR 0024). OpenObserve, as deployed,
is HTTPS-only on a public Cloud Run URL.

So the drain cannot reach it. A root given the public URL refuses to start
with `EX_CONFIG` — which is the correct refusal, and is why setting the
variable today would take down the service rather than export anything.

The only TLS-terminating hop this platform has is the egress proxy. That
gives the shape of the work, and one hard problem inside it.

### 0.2 Steps

**0.2.a — Add OpenObserve as an egress upstream.**
Ports 9101–9105 are taken (`infrastructure/egress/envoy.yaml`); **9106 is
free**. Add a listener on 9106 and a cluster to the OpenObserve host, and add
that host to `egress_allowed_upstreams` in
`infrastructure/terraform/variables.tf`. The acceptance suite fails if the
variable and the bootstrap disagree in *either* direction, which is the guard
that keeps this honest — update both in one commit.

*Evidence required:* the egress acceptance suite, plus a test asserting 9106
routes to the OpenObserve host and to nothing else. Mutation-verify by
repointing the cluster.

**0.2.b — A credential for the drain, as a file.**
`QIP_OPENOBSERVE_AUTHORIZATION` carries the whole `Authorization` header
verbatim. These are **built** binaries, so ADR 0031's `secret_env` is refused
for them and must stay refused: use `secret_mounts` and the `_FILE`
indirection `qip_core::secret` already supports. Add one secret
(`qip-openobserve-drain-authorization`) to `main.tf`'s list; `infra.yml`'s
seeding step will mint a first version, but the value must be a real
OpenObserve token, so seeding a random string is wrong here — this one is
written by an operator or minted from the root login.

*Note the ordering trap this session already hit once:* a `secret_mounts`
entry grants `secretAccessor` through
`google_secret_manager_secret_iam_member.mounted`, and the service now
`depends_on` both grant resources. That is already fixed; do not re-introduce
an unordered grant.

**0.2.c — Set the variables, per root, in `catalogue.tf`.**
`QIP_OPENOBSERVE_URL=http://127.0.0.1:9106`, `QIP_OPENOBSERVE_ORG`
(required whenever the URL is set — OpenObserve scopes ingestion paths as
`/api/{org}/...` and there is no defensible default),
`QIP_OPENOBSERVE_INTERVAL_SECS` explicit rather than defaulted.

**0.2.d — Update `manifest_wiring.rs`.**
Its `READ_BUT_NOT_SET` list is the record of why these are unset. Moving them
out of it is part of the change, not a follow-up, and the suite is what
notices if only one of the two happens.

**0.2.e — Prove ingestion, do not assume it.**
The deliverable is a query against OpenObserve returning a series this
platform emitted, with the timestamp and the value. Not "the drain thread
started" — that is a log line, not evidence. Until that query returns rows,
the platform is not observable and no document may say it is.

### 0.3 The fast brain problem — a decision, not a task

**`qip-fastbrain` deliberately has no egress proxy.** ADR 0024 puts a sidecar
beside the API and the deep brain and not beside the fast brain, and
`catalogue.tf` refuses at plan time to give it one — because port 9102 on
that proxy routes to a language model API and nothing on the fast path may
consult a model (ADR 0008).

Consequences, all real:

1. The fast brain has no loopback hop that terminates TLS, so its drain
   cannot reach an HTTPS collector at all.
2. Giving it the existing proxy would hand the fast path a route to a model
   API. That is not a configuration change; it is the erosion of ADR 0008.

Three ways out, and the choice is the owner's:

- **(a) A second, narrower proxy for the fast brain** carrying exactly one
  upstream — OpenObserve — and no model route. Keeps ADR 0008 intact by
  construction rather than by policy. Costs a second sidecar definition.
- **(b) A plaintext collector inside the VPC.** The fast brain reaches it in
  the clear with no proxy at all. This is the shape the drain's own module
  documentation says "a deployment would have to provide". Costs the
  vendored-and-attested collector image that A6 has been blocked on anyway,
  and it makes the pull path free at the same time.
- **(c) Accept the fast brain is not drained.** Its telemetry stays local and
  its `/metrics` is scraped by the pull path only. Cheapest, and leaves the
  latency-sensitive plane the least observed — which is the plane where a
  problem is most expensive.

**Recommendation: (b).** It closes the fast-brain gap, the A6 collector gap,
and the pull path together, and it removes the public-internet hop from the
telemetry route entirely. (a) is a legitimate second choice if vendoring the
collector stalls again.

### 0.4 A security question this raises, which should be answered before 0.2.c

Routing telemetry through the egress proxy to OpenObserve's *public* URL
means the platform's operational metrics cross the public internet to an
endpoint that accepts anonymous connections. ADR 0030 accepted anonymous
*read* exposure on an empty service. It did not consider the service holding
the platform's operational history, nor anyone else being able to POST into
it.

This is exactly the trigger ADR 0030 named for itself: "the service is empty
today and stops being empty the moment any deployment sets
`QIP_OPENOBSERVE_URL`. That change is the one that must move this behind IAP
or re-argue the exposure."

**So 0.2.c fires ADR 0030's own re-argument condition.** Either move
OpenObserve behind IAP before the first byte of telemetry lands, or record a
new ADR accepting a publicly-anonymous store of operational history. Option
(b) in §0.3 sidesteps this by keeping telemetry inside the VPC — a third
reason to prefer it.

---

## Track A — Gate: End of Phase 2

> *Does a family survive holdout with honest significance after cumulative
> trial correction?* — **NOT PASSED**

### What is already done

More than the score suggests. `qip_lifecycle::trials::TrialBook` keeps one
hash-chained journal per family; `lifetime_trial_count_known` refuses a
promotion whose count is unknown; every root opens the book durably on its
own store; the book budgets five hundred trials per family per calendar
quarter; `HoldoutBand::from_deflated` exists and is two-sided at the demotion
monitor. §20.1's accounting is ALIGNED in code and mutation-tested.

**Nothing in this track is a code gap.** The gate is blocked on one sentence
in its own evidence: *"it is an empirical question about real market data,
and every deployment's data is synthetic or replayed. A family surviving a
holdout of data the platform generated is not the gate."*

### A.1 A real vendor feed, reaching a deployed process

Partly closed and unrecorded anywhere: `api.frankfurter.app` is now the sixth
entry in `egress_allowed_upstreams`, a real Envoy cluster on 9105, with
`FrankfurterRatesConnector` registered in the connector bridge. No scoring
document mentions it.

What is still missing:

- **No request through that allowlist has been observed in a log.** The
  connector is wired; nothing proves a response ever came back in a
  deployment. First deliverable: one request, one response, quoted.
- **Frankfurter is FX reference rates.** It is not an equities or crypto
  market-data feed, and the universe the platform sizes against is equities.
  A family surviving a holdout on FX rates answers the gate for FX and for
  nothing else.

So: **name the equities/crypto vendor** (row D9, an owner decision), evaluate
its licensing posture in `qip-data-finder` *before* it reaches the catalogue —
that gate is not optional and must refuse a research-only licence — then add
the host to the allowlist and the bootstrap together.

### A.2 Seven days of stable streaming

Row B4. The requirement is continuity, not volume: seven days during which
ingestion does not stall, the bitemporal record stays consistent, and the
statistics converge. This cannot be compressed and cannot be simulated —
replayed data is what the gate already excludes.

**Preconditions before the clock starts,** or the seven days will have to be
re-run:

1. Track 0 complete and proven ingesting, so the run leaves a record.
2. `workload_metrics_exist` flipped — legitimately, on evidence something
   scraped — so a stall pages someone instead of being discovered on day
   seven.
3. Bounded retention verified under sustained load, not just under test.
   `qip-streaming`'s bounds are asserted where they are enforced; they have
   never run for a week.

### A.3 Run the gate and record the answer

With `TrialBook` correcting cumulatively across the whole run, evaluate a
family against a genuine holdout. **Record the answer whichever way it
falls.** A family that fails the gate on real data is a passing *process* and
a successful outcome for this track; the blueprint's "If no: Stop" is an
instruction to stop promoting, not to stop reporting.

### A.4 Dependencies

`A.1 → A.2 → A.3`, with Track 0 blocking A.2. Everything in Phases 4–19 is
blocked on A.3.

**Honest estimate:** the code work is small — days. The gate is at least
seven days of wall-clock after the last precondition lands, and realistically
longer, because the first sustained run will surface things a test suite
cannot. Do not plan A.2 as if it succeeds first time.

---

## Track B — Gate: End of Phase 6

> *Is calibrated probability better than the market's implied on prediction
> contracts?* — **NOT PASSED**

`qip-prediction` has `market.rs`, `oracle.rs`, `pricing.rs`, `resolution.rs`,
and `pricing.rs` already computes `implied_from_bid/ask`. Confirmed by grep:
**no comparison of the platform's probability against a venue's implied
probability exists anywhere.**

### B.1 The scoring itself

Implement Brier scoring of the platform's calibrated probability against the
implied probability from the same contract at the same instant. Both terms
already exist; nothing computes the difference. This belongs in
`qip-prediction`, is pure arithmetic over existing types, and needs no
dependency.

Two things that will otherwise make the result worthless:

- **Point-in-time discipline.** The implied probability must be the one
  knowable at the instant the platform's probability was formed. The
  bitemporal record exists precisely so this is answerable; using a later
  quote is leakage and the number would be a fabrication.
- **A baseline.** Brier alone is not a comparison. Score the platform, score
  the market, and report the difference with its uncertainty.

### B.2 A prediction venue

The gate names *prediction contracts*. This platform has no prediction-market
data source. That is a second vendor decision (and a second licensing
evaluation), separate from A.1's equities feed.

**This is the reason B is genuinely a separate track and not a sub-task of A**
— it needs its own source, and it can be built and tested against recorded
contract data while A.2's seven days run.

### B.3 Dependencies

`B.1` can be written now, against fixtures, and should be — it is the one
gate whose *machinery* is missing rather than whose data is. `B.2` and the
gate itself follow Track 0. B does not block A and can run in parallel.

---

## Track C — Gate: End of Phase 8

> *Does regime-conditional allocation beat unconditional out of sample?* —
> **NOT PASSED**

Regime detection exists (`qip-cost-router/src/context.rs`,
`qip-simulation-engine/src/conditions.rs`). Confirmed by grep: the only
"unconditional" hits in the tree are a degradation test and doc comments.
**No out-of-sample comparison against an unconditional baseline is computed.**

### C.1 The unconditional baseline

There is no such allocator today. Build one — the same sizing with the regime
term removed — as a first-class comparison arm, not a test fixture. This is
the same discipline ADR 0006 imposes on the quantum path: a classical
baseline computed every time, because "we used a regime model" is not a
result.

### C.2 Out-of-sample split, declared in advance

The split must be fixed before the comparison runs and recorded in the event
log with everything else, or the result is unfalsifiable. Reuse the holdout
machinery Track A already relies on rather than inventing a second notion of
out-of-sample — two definitions of a holdout in one platform is a
reconciliation break waiting to happen.

### C.3 Dependencies

`C.1 → C.2 → the gate`. C.1 is buildable now. The gate needs A.2's data,
because "out of sample" over synthetic data means nothing. **C is where the
per-region reservation and the unwired edge plane become load-bearing** —
allocation is the cell's job and no deployed cell runs (`execution_nodes = {}`
in all four environments), so C also needs the execution-node deployment
decision, which is still open.

---

## Track D — Gate: End of Phase 3 (cannot pass)

> *Does it survive contact with a live venue, inside its holdout band?* —
> **CANNOT PASS**

Report it as structurally refused, not outstanding. Three layers enforce
paper trading — Terraform refuses a live ceiling at plan time,
`AutonomyLevel::deployable` stops the process at start-up, and `qip-edge`'s
`Cell` has no constructor taking any other ceiling. None may be weakened.

What would change it is ADR 0023's sequence, which is written and whose steps
are **not approved and have not been requested.** Opening it is a governance
decision with capital at risk, taken by a person, in writing, and is out of
scope for any engineering plan including this one.

The band the gate asks about now exists (`HoldoutBand::from_deflated`,
two-sided at the demotion monitor), so if the boundary is ever opened there
is something to be inside of. That is the whole of what engineering can
usefully do here, and it is done.

**Action: none. Re-classify in the traceability document so it stops reading
as work.**

---

## The six decisions — all taken

All six were delegated and are recorded. Nothing in this plan is now waiting
on an answer; what remains is execution.

| # | Decision | Taken | Record |
|---|---|---|---|
| 1 | Fast-brain telemetry route | **(b) in-VPC plaintext collector** | ADR 0032 |
| 6 | A6's collector image | **Same image.** Subsumed by 1 | ADR 0032 |
| 2 | OpenObserve exposure once it holds data | **Authenticated, still external.** `allUsers` ends | ADR 0033 |
| 3 | Market-data vendor | **Coinbase, then Alpaca** — transport proven before contract | ADR 0034 |
| 4 | Prediction source | **Kalshi** | ADR 0034 |
| 5 | Execution nodes | **One. `us-east4`, shadow mode, dev only** | ADR 0035 |

Three consequences change the plan above rather than merely answering it:

- **§0.2.a and §0.2.c are superseded in their routing.** No root drains to
  OpenObserve's public URL through the egress proxy on 9106; every root
  drains to the collector on a private address. The 9106 listener is not
  needed and should not be built. What survives from §0.2 unchanged is the
  credential as a file mount (0.2.b), the per-root variables (0.2.c, pointed
  at the collector), the `manifest_wiring.rs` update (0.2.d), and the
  requirement to prove ingestion with a query (0.2.e).
- **§0.4's security question is answered by two decisions at once.** ADR 0032
  keeps telemetry off the public internet entirely, and ADR 0033 ends
  anonymous access to the store regardless. The `open-anonymous` posture
  stays in `modules/cloudrun` as a tested, refusable capability with zero
  users, and the acceptance suite's anonymous set becomes empty.
- **Track C is unblocked at its infrastructure end.** ADR 0035 authorises the
  one node Track C needs, with the explicit limit that one region cannot
  exercise partition behaviour or cross-region reservation contention — so
  C's result must be scoped to what a single cell can honestly support.

The ordering constraint ADR 0035 sets is worth lifting out, because it
reverses the obvious sequence: **the collector must be ingesting before the
node is deployed.** Standing up the least-observed subsystem in the platform
with nothing watching it is the exact failure that decision exists to end.

---

## Sequencing

```
Track 0  ──────────────────────────────────────────►  everything
  0.1 blocker understood ─► 0.2 push path ─► 0.5 prove ingestion
        │                        └─ decisions 1, 2 taken (ADR 0032, 0033)
        └─ the collector also closes A6 and the pull path
              └─► then, and only then, the one node (ADR 0035)

Track A   A.1 vendor ─► A.2 seven days ─► A.3 GATE 2 ─► Phases 4-19
                            ▲
                            └── requires Track 0 proven

Track B   B.1 Brier machinery (buildable NOW, in parallel)
              └─► B.2 venue ─► GATE 6

Track C   C.1 unconditional baseline (buildable NOW, in parallel)
              └─► C.2 declared split ─► GATE 8
                        ▲
                        └── requires A.2 data + decision 5

Track D   no action; re-classify as structurally refused
```

**Start immediately, in parallel, needing no decision:** B.1 and C.1. Both
are self-contained arithmetic over types that already exist, both are
currently *missing machinery* rather than missing data, and both will be on
the critical path the moment data arrives. Building them during A.2's seven
days is the difference between one gate attempt and three.

**Start as soon as decisions 1 and 2 land:** Track 0.

## What this plan refuses to promise

- **A date for Gate 2.** It is seven days of streaming *after* an unknown
  amount of first-run instability, following a vendor decision nobody has
  taken.
- **That any gate passes.** Each is a real question. Building the machinery
  to ask it honestly is the deliverable; the answer is the market's.
- **That 4 of 4 is reachable.** It is not, while paper trading holds, and
  reporting otherwise would be the kind of false statement about the state of
  the system this repository exists to prevent.
