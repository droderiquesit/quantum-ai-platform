# Final system report

What this platform is, subsystem by subsystem, with the evidence for each
verdict and the evidence's limits.

---

## The verdict

**This platform is not ready to run real money, and nothing in this repository
supports a claim that it is.**

(The phrase a reader might expect in that sentence does not appear anywhere in
this repository, in the affirmative or the negative.
`crates/tests/qip-acceptance/tests/documentation.rs` greps every document for
it and fails. That is deliberately blunt: a checker clever enough to allow the
negated form is a checker that will eventually get the negation wrong, and the
cost of the bluntness is one awkward sentence.)

That is not a hedge. It follows from four facts, each of which is
independently sufficient:

1. **Nothing has been deployed.** This build environment has no `gcloud`, no
   `terraform`, no credentials and no application-default credentials. The
   infrastructure has never been planned, never validated against a provider
   schema, and never applied. Twenty-six Terraform files and six Kubernetes
   manifests are *specified and structurally tested*; they have not been run.
2. **The platform has never connected to a venue.** There is no market-data
   transport and no order-entry transport in this build. Every fill in every
   test is simulated, and the flag saying so is set by the gateway rather than
   by the order.
3. **No end-to-end latency has been measured.** Not wire-to-wire, not
   tick-to-order, not cross-region. `docs/performance/budgets.md` measures
   eight stages in isolation and opens with a section saying, at length, that
   none of it is a latency figure for an assembled system.
4. **The quantum work runs on a simulator.** `qip-quantum` contains a
   statevector simulator and a QAOA implementation on top of it. The IBM
   Quantum port reports itself unavailable and names the three things it needs.
   Simulating a QAOA circuit costs more than solving the problem it encodes, so
   running one proves the formulation is right and proves nothing about
   advantage.

What the platform *is*, stated as precisely: **a complete, internally
consistent, heavily-tested implementation of the decision system, with every
external boundary present as a port that refuses rather than pretends.** That
is a real thing and it is most of the work. It is not a running system.

---

## The evidence

| | |
|---|---|
| Crates | 57 |
| Source lines | 120,864 |
| Test lines | 63,527 |
| Tests passing | **2,043** |
| Tests failing | **0** — `make count`, which exits non-zero if any do |
| `unsafe` blocks | **0** — `unsafe_code = "forbid"` workspace-wide; the thirteen occurrences of the word are all prose |
| Third-party packages in the lockfile | 11, all permitted (`serde`, `serde_json` and their closure) |
| Clippy | clean, `--workspace --all-targets`, warnings denied |
| Formatting | clean, `cargo fmt --all --check` |
| Terraform | 26 files, 2,191 lines — never applied |
| Kubernetes manifests | 6 — never applied |
| Architecture decision records | 10 |

Reproduce all of it with `make check`, and count it with `make count`.

**Count it with `make count`, not with a grep.** Summing `test result: ok. N
passed` misses a failing binary entirely — a failing target prints `FAILED`
instead and contributes nothing — so the total comes back lower and still
looks like a clean number. That is the worst way for a measurement to be
wrong: it under-reports rather than erroring, and a report written from it
says the suite is green. It said so here, wrongly, for one revision.
`scripts/count-tests.sh` counts both outcomes and both columns, exits on
cargo's status rather than on the parse, and refuses to hand back a passing
count on a red suite without the failing one beside it.

**What a passing test proves here.** The house convention is that a test
asserts a *property* rather than restating an implementation, and that a
safety test asserts the unsafe thing is refused rather than that the safe path
works. Several of the defects listed at the end of this document were found by
tests written that way, in code that already passed a suite written the other
way. That is the argument for the number; it is not an argument that 1,998 is
a large number.

---

## Subsystem verdicts

**PASS** means: implemented, tested against properties, and the boundary
conditions refuse. It does not mean deployed, connected, or benchmarked
against production load. Read the caveat column, which is doing real work in
several rows.

### Layer 1 — Autonomous data finder and the data mesh

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| Autonomous data finder | **PASS** | 62 | Discover → classify → probe → assess legality → score → route → register → monitor → drift → replace. Legality is three-valued and combines by least-permissive, so an absent robots.txt and an unreadable licence both produce `Unknown` and `Unknown` never collects. A source scoring 1.0 on everything and forbidden is rejected, and no path through the type reaches another class. **Opens no sockets**: `NetworkProbe` reports `Unavailable` naming what production must supply. |
| Market ingestion | **PASS** | 30 | Adapters, replay, provenance on every record. No venue transport. |
| Normalisation | **PASS** | 17 | Symbol mapping, unit conversion, quality stamping. |
| Entity resolution | **PASS** | 21 | |
| Streaming / durable log | **PASS** | 52 | Hash-chained append-only journals, sequencing, tiered storage, a Pub/Sub port. The port does not reach Pub/Sub. |
| Storage | **PARTIAL** | 10 | Blob and catalogue interfaces exist. BigLake, BigQuery, Bigtable, Spanner and AlloyDB are named ports with no client behind them — see `docs/operations/external-dependencies.md`. |

### Layer 2 — Regional AI brains (the edge cell)

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| Protocol decoding | **PASS** | 48 | Wire formats, diagnostics, skipped-message accounting. |
| Sequencing | **PASS** | 33 | Gap detection, reorder policy, abandonment. A gap marks the book stale, and a stale book serves no price. |
| Order book | **PASS** | 41 | Built only from messages; there is deliberately no setter that bypasses the feed. |
| Feature DAG | **PASS** | 23 | Dirty-marked recomputation; "the vector does not carry it" is a different failure from "it is undefined", and the runtime keeps them apart. |
| Strategy compiler and runtime | **PASS** | 32 | Compiles to a node arena with a proved worst-case cost. |
| Arbitrage | **PASS** | 24 | Triangular search, path pricing, leg planning ordered hardest-to-undo-first with a written rationale. Nine deductions, and a net figure its parts must sum to. |
| Routing | **PASS** | 26 | |
| Edge cell composition | **PASS** | 20 | Decides alone under a signed, bounded, venue-scoped, expiring envelope. Refusals are journalled like decisions. Hot path does no I/O. **A cell can no longer hold a strategy it cannot evaluate** — deployment takes the program the plan indexes into and refuses a mismatch. |

### Layer 3 — Global opportunity brain

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| World model | **PASS** | 33 | Bitemporal. A bar becomes known no earlier than its close, which is a look-ahead defect that was found and fixed. |
| Opportunity engine | **PASS** | 22 | Detectors find an 8.5% jump in a 0.9% series in the end-to-end run. |
| Reasoning engine | **PASS** | 41 | |
| Investment agents | **PASS** | 33 | The organisation reaches the desk; the desk is read-only, and no agent crate can reach the execution engine — asserted over the parsed dependency graph. |
| Prediction | **PASS** | 36 | |
| Simulation | **PASS** | 75 | Backtest, Monte Carlo with antithetic variates, scenarios, purged k-fold with embargo, walk-forward, deflated Sharpe. Square-root impact law, and a refusal to price beyond the participation it was calibrated for. |
| Optimisation | **PASS** | 24 | Classical and quantum-inspired, with a compute router. |
| Quantum | **PARTIAL** | 35 | Statevector simulator, QAOA, three solvers behind one trait, and a benchmark that **cannot produce a report without a classical baseline** and whose only usable answer is one re-evaluated classically. The IBM port refuses and names the token, the CRN and the transport. **No hardware has been reached and no advantage has been measured.** |

### Layer 4 — Capital brain

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| Allocation and envelopes | **PASS** | 19 | Sizes on the lower confidence bound, not the point estimate. Every envelope is signed, venue-scoped and expires within twelve hours, because expiry is the only revocation a disconnected cell will honour. Issuing needs a dual approval. |
| Predictive capital fabric | **PASS** | 26 | Pre-positions against a demand forecast; settlement conventions, funding curves, and an explicitly asymmetric shortfall cost. |
| Cost engine and model router | **PASS** | 18 | The cost of having decided is one of the nine deductions, not an afterthought. |
| Portfolio construction | **PASS** | 22 | |

### Layer 5 — Regional execution mesh

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| Risk engine | **PASS** | 33 | Deterministic pre-trade limits. Raising autonomy needs two operators with fresh credentials *and* a ceiling that permits it, and the default ceiling permits nothing live. |
| Execution engine / OMS | **PASS** | 28 | Reconciliation breaks are counted and surfaced rather than discarded. |
| Broker and venue adapters | **PASS** | 50 | `AdapterClass` has `Simulated` and `Sandbox` and **no `Live` variant**, so a live adapter does not compile and no configuration string deserialises into one. Secrets have hand-written redacting `Debug`/`Display` and no `Serialize` at all. Price-time priority is checked against an independently computed trade sequence over 400 generated books. **There is no sandbox adapter yet** — the class is representable and only the simulated exchange implements it. |
| Drop-copy reconciliation | **PASS** | (in edge) | A partial fill is reported as a break rather than rounded up to the order. |

### Layer 6 — Outcome capture and the counterfactual twin

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| Outcome capture | **PASS** | 30 | Every order, fill, cancel, rejection, missed opportunity and risk ruling, hash-chained, each carrying a trace id. A refusal with no row would be indistinguishable from a decision nobody made, so refusals are captured. |
| Counterfactual twin | **PASS** | (same) | A simulated figure **cannot** be added to a realised one: there is no accessor out of `Simulated`, no `Deref`, no `From`, no general `map`, and Rust's orphan rules mean no other crate can add the impl. The taint survives serialisation. An alternative is planned against a view with no timestamped accessor and settled only after the plan is fixed. The fill model is the simulator's, not a second more generous one. |
| Chain of custody | **PASS** | 34 | |

### Layer 7 — Evolution and learning

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| Learning engine | **PASS** | 32 | Attribution reconciles to an exact residual. |
| Evolution brain | **PASS** | 54 | The multiple-testing discipline is structural: the trial count is a type with a private field and no constructor, obtainable only by folding real runs into a ledger, so a caller cannot declare one trial after five thousand. A challenger takes a return series that cannot be built without a matching cost series. The headline test runs two real searches — five candidates and five thousand — and shows the same challenger at Sharpe 3.2 winning one and losing the other. |
| Training and distillation | **PASS** | 54 | A student is fitted to its teacher and never to the labels, asserted by distilling against two probe sets whose targets differ by a factor of −100 and requiring byte-identical students. A fit on pure noise reaching an in-sample R² above the bar is refused on the holdout. **Vertex AI is a port that refuses**, naming the transport it lacks; it does not fall back to local training quietly. |
| Lifecycle and gates | **PASS** | 26 | |

### Cross-cutting

| Subsystem | Verdict | Tests | Evidence and caveat |
|---|---|---:|---|
| Compliance controls | **PASS** | 65 | Six controls, each stated as the unsafe action and the mechanism that makes it impossible or refused. A report whose signing key is reproducible says so in the report. |
| Contracts | **PASS** | 41 | Exact `Decimal` for money, bitemporal stamping, attributed origin, and a `NetEdge` that refuses a total its parts do not sum to. |
| Observability | **PASS** | 21 | |
| API surface | **PASS** | 53 | Constant-time token comparison with no early return, bounded allocation for unauthenticated callers, one route table that is the whole surface, and an OpenAPI document **generated from that table** rather than maintained beside it. A surface with nothing behind it names the reason and returns no number at all. |
| Web console | **PASS** | 31 | Server-rendered, no JavaScript, `default-src 'none'`. It can trip the kill switch and has no path that clears one. |
| Kernel / composition root | **PASS** | 28 | A cycle never panics and never stops early; a failing stage records why and the loop continues. |
| Determinism and replay | **PASS** | 66 | No ambient clock, no ambient RNG; injected `Clock` and seeded `Xoshiro256`. Bit-exact replay. |
| Infrastructure as code | **PARTIAL** | 54 | 54 structural tests assert properties a plan would not catch — the node pool has no public addresses, no workload identity holds delete on the evidence bucket, no credential appears in any manifest, every binary the workspace builds is either deployed or excluded by a named decision. **Never validated against a provider schema and never applied.** |
| End-to-end demonstration | **PASS** | 1 | `crates/tests/qip-acceptance/tests/e2e.rs`: one run from a discovered source through ingest, a regional brain, the global loop, a three-arm dislocation, an allocation, a dual approval, a signed grant the cell verifies itself, an order, a partial fill reported as a break, an outcome on a hash chain beside six alternatives that cannot be added to it, and a solver benchmark whose quantum arm names the credential it lacks. Its own module doc states what it does not prove. |
| Chaos and stress | **PASS** | 16 + 16 | Every test names the specific safe outcome the failure has to produce. "Nothing crashed" is not asserted anywhere, because it is the assertion that lets a system which silently invented a number pass. |

---

## What is not built

Named rather than implied, because an absence nobody wrote down is a
surprise later.

* **Every managed Google service.** Pub/Sub, BigQuery, BigLake/Iceberg,
  Bigtable, Spanner, Spanner Graph, AlloyDB, Memorystore and Vertex AI are
  ports. Each names its exact requirement in
  `docs/operations/external-dependencies.md`. ADR 0009 permits the clients at
  the I/O edge; none has been written.
* **IBM Quantum.** Needs a token, a service-instance CRN, and an HTTPS
  transport with a Qiskit Runtime client. None is present.
* **Any hosted language model.** Same reason: no HTTP client with TLS.
* **A sandbox venue adapter.** The class is representable and nothing
  implements it.
* **Venue market-data and order-entry transports, and a drop-copy feed.**
  `qip-edge-node` prints the three it awaits on start-up and serves its health
  surface without them, trading nothing. That is the correct degraded state and
  it is not a connection.
* **An intake for cell reports.** Nothing in the running API ingests a
  `CellReport`, so a deployed `qip-api` would truthfully answer "no edge cell
  has reported" on every regional surface. Adding one is a new mutating route
  with real authority implications — a design decision, not a repair.
* **Self-trade prevention in the simulated exchange.** A client crossing its
  own resting order books both legs: the position nets to zero and commission
  is charged twice. The books still reconcile exactly, so this is an
  economic-realism gap rather than an accounting break.

---

## Defects found and fixed, as evidence about the method

These were found by tests asserting properties, in code that already passed a
suite. They are listed because they are the argument that the test count means
something.

| Defect | Why it mattered |
|---|---|
| The simulator priced a slice against the touch read back *after* the sweep | The order's own market impact sat inside the reference instead of beyond it, so the impact term double-counted and a ten-times slippage regime multiplied by about seven |
| A crossed book was filled at the worse of its two touch prices | The book is built symmetrically about the mid, so at any cross width *both* quotes are inside the calm touch — the "worse" one included. Charging it is still charging less than an orderly market, which turns a data fault into a subsidy a backtest will learn to seek out |
| `WorldModel::absorb_bar` accepted a known-time before the bar closed | Look-ahead: a backtest could read a price that had not printed |
| `Cell::deploy` did not take the program its plan indexes into | `NodeRef` is an index; in a large enough arena it resolves to another strategy's node and the cell emits a signal from arithmetic nobody wrote for it |
| `serde_json` declared without `float_roundtrip` | Every content digest over an `f64` was a within-process identity only; two copies of one model that took different routes through JSON did not collide |
| `TrialCount` was `Option<usize>` on an all-public struct | A caller could declare one trial after searching five thousand — the entire multiple-testing discipline, defeated by a struct literal |
| Nothing forced net-of-cost scoring in evolution | A gross-alpha promotion looks exactly like a real one |
| `TrainedTeacher::structure_digest` omitted the intercept, base and learning rate | A structure update replacing one two-coefficient form with another produced the same digest — the thing the digest exists to catch |
| `split_at_fraction` called `clamp(1, len - 1)` | `clamp` asserts `min <= max`, so a one-row dataset aborted the process from a submitted job |
| `KillSwitch::clear_global` discarded its operator identity | An anonymous resumption of a halted platform |
| The OMS discarded the `Result` of `apply_fill` | Reconciliation breaks vanished silently |
| `LegPlan::residual_after` summed across currencies | A stranded exposure reported as one number that was three |
| The kernel retained every proposal ever made | Unbounded growth in the long-running process |
| The refusals console panel was modelled as a metric | "Refusals 3" cannot say whether a risk limit or an unreachable venue caused them, and only one of those may be retried |

---

## What would have to be true before live money

In order. None of these is a matter of finishing the code.

1. **Apply the infrastructure**, from a human identity impersonating the
   bootstrap account — never a downloaded service-account key. See
   `docs/security/credentials.md`.
2. **Write the venue transports** and reach a sandbox venue. Then reach it for
   long enough to have a drop-copy history to reconcile against.
3. **Measure end-to-end latency** on the deployed path and rewrite
   `docs/performance/budgets.md` from what is measured rather than
   interpolated.
4. **Run in shadow**, with the counterfactual twin recording what the platform
   would have done, for long enough that the regret distribution is informative
   rather than noise.
5. **Raise the autonomy ceiling deliberately**, as a reviewed change to a
   `terraform.tfvars`, with two operators, fresh credentials, and a named
   scope. Supplying a broker credential does not enable live trading and is
   not intended to.

Each of those is a gate, not a step. Passing the one before it is what makes
the next one meaningful.

---

*Regenerate the evidence in this document with `make check`; the per-subsystem
counts come from `cargo test -p <crate>`.*
