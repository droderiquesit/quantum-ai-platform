# ADR 0087: A causal edge that fails its conditions three passes running is retired from inference and never re-estimated back

- **Status**: Accepted, under the authority the owner delegated on 2026-09-19
  to the lanes deciding in-territory records (the precedent is ADRs 0081
  through 0085, each accepted in the lane that wrote it). Built by the same
  lane in the commits this record travels with: `qip-world-model/src/causal.rs`,
  `world.rs`, `state.rs` and `tests/retirement.rs`. No kernel line changed;
  the seam the kernel already calls is where the decision lives.
- **Date**: 2026-09-20
- **Supersedes**: nothing. Closes the one clause the register held §9.4 short
  on, and corrects one thing the prior lane's own code comment implied: that
  "retired" could be answered by `in_regime` reading a failure over a stale
  hold. That ordering is right and is kept; it is a *reading*, and the
  blueprint's word is a *verb*.
- **Related**: ADR 0005 (confidence is arithmetic), ADR 0007 (attribution),
  ADR 0062 (a venue is withdrawn on evidence and reinstated only by two
  signatures — the shape of "retired, not patched" this platform already has
  for venues), the §9.1 conditions layer landed in `3dacf04`, and the causal
  digest producer landed in `d5c6266`.

## Context

Blueprint §9.4 answers "financial markets are non-stationary" with: "Edges
are conditioned on regime and re-estimated. An edge that fails its conditions
is retired, not patched." The first two clauses were built on 2026-09-19:
`CausalEdge` carries `holds_in` and `fails_in`, both written in production by
the temporal-precedence pass (`grep -n 'with_conditions(BTreeSet::from\|record_causal_condition_failure' backend/crates/runtime/qip-kernel/src/platform.rs`),
and `CausalGraph::reestimate` re-scores every edge from the claims inside the
contract's horizon. The third clause had nothing behind it. A failure wrote
`ConditionStanding::KnownToFail` and every reader of the graph — propagation,
explanation, second-order exposure, the episode's causal context — went on
using the edge exactly as before. The platform's own test could refuse a claim
on every pass for a year and change nothing the platform did with it.

The prior lane left that deliberately, and its reason stands: withdrawing an
edge from `propagate`, `incoming` and `explanations` changes the semantics for
every reader of the graph, and a change of that reach is a decision, not an
implementation detail. This record is the decision.

Two facts about the pass shape what can honestly be decided. First, the pass
re-runs a pair's test every cycle over `log_returns(price_history)`, a window
that slides by one bar per cycle, so consecutive results share almost all of
their data and are not independent refutations. Second, the pass writes a
*pass* as a new edge carrying `holds_in = {regime}` rather than marking the
edge it already holds, so "consecutive" has to be defined against a signal
that arrives as an insertion, not a mark.

## Decision

### 1. What "fails its conditions" means

An edge fails its conditions when its own test has been recorded failing
`RETIREMENT_CONSECUTIVE_FAILURES` times running under one regime key with no
hold recorded for its pair in that regime between them. The constant is
**three**, declared in `causal.rs` and pinned by a test that names this
record.

Three is a debounce, and this record says so rather than dressing it as a
significance level. One failure is a sighting, and it already writes
`KnownToFail` — §9.4 names "known to fail" and "retired" as two things, and a
count of one would make them one event. Two is the same window one bar on,
which can still be a single gap or corporate action in the tape. Three is a
run: the test has refused the claim on every pass since the run began, and a
hold in between would have reset it. The number that would make consecutive
results independent is the window length, and a run that long would outlive
most regimes; that is the first condition under "What would make this wrong", not a reason to pick it.

Per edge, one `FailureRun { regime, failures, began }`. A failure recorded
under the run's regime extends it; a failure under any other regime starts a
fresh run at one. So a regime boundary resets the count, nothing carries
across it, and a regime the edge has never been observed in starts from zero
— the first-sighting rule `qip-kernel/src/regime_transition.rs` uses for a
crossing, applied to a refutation. A regime the platform has never labelled
cannot appear at all: the key arrives from the kernel's `regime_context` of
the *effect*, which names the regime in force on that tape, and a blank key
is refused before anything is written. Nothing here invents a regime.

A hold breaks the run. `CausalGraph::add` clears the run of every live edge
of the same pair whose run regime is in the new edge's `holds_in`. Without
this "consecutive" would mean "cumulative" and a link that clears its bar
every other cycle would retire on its third miss.

Failures are recorded pair-wise, for every mechanism the pair carries; that
was the prior lane's decision and retirement follows the record rather than
adding a second rule. A hand-asserted supply-chain claim the precedence test
refuses three passes running is retired. "What it costs" names the cost.

### 2. What retirement does

- **The edge leaves inference from the instant it retired.**
  `CausalGraph::outgoing` and `incoming` filter on
  `CausalEdge::retired_by(known_at)`, so `propagate` and `explanations`, and
  through them the kernel's `causal_context` (the episode's causal edges),
  `second_order_exposure`, `hidden_concentration`, `unheld_dependencies` and
  `instruments_exposed_to`, stop returning it.
- **Point in time holds in the direction the domain rule does not usually
  have to state.** A reader asking about an instant *before* `Retirement::at`
  still sees the edge, because at that instant it was live; a backtest that
  saw Wednesday's refutation when asked about Monday would be reasoning from
  a graph it did not have, and the results would look better rather than
  anomalous. `retired_by` is `at <= known_at`, and a test holds both sides.
- **It is never re-estimated back.** `reestimate` skips a retired edge: no
  strength moves, no decay is marked, and the claims that would have matched
  it fall through to `unmatched` unless a live edge holds the same link.
- **It stays in the graph.** `edges()` and `len()` still hold it, under
  `EdgeStanding::Retired`, with `Retirement { regime, at,
  consecutive_failures, run_began }` on the edge. Retired, not deleted: the
  record of what was claimed, what refuted it and when is what a reviewer
  judges the re-established link against.
- **A re-established link is a new edge with new evidence.**
  `CausalEdge::validate` — the check every path admitting an edge to a graph
  meets — refuses an edge carrying a retirement, and refuses one carrying a
  run, so a claim cannot arrive already retired or already two failures in.
- **It is journaled and counted.** `WorldModel::record_causal_condition_failure`
  pushes one `Change` per retirement under the new
  `ChangeKind::CausalEdgeRetired` — its own kind, because a revised belief is
  still held and a retired edge is not — and `statistics()` reports
  `causal_claims_retired` beside a `causal_claims` total that does not move.
  The kernel's seam is untouched: the function still returns the marked
  count `platform.rs` adds into its stage detail. Freshness still does not
  move; a retirement is the graph losing a claim, not absorbing one.

### 3. What retirement deliberately does not do

- **It never fires on a first sighting**, in any regime, for the reason in
  §1.
- **It releases nothing a whole-graph reader constrains.** Readers that
  iterate `CausalGraph::edges` keep seeing a retired edge: the shared-cause
  exposure producer (`qip-kernel/src/shared_cause.rs`) that caps the book per
  causal driver, the causal audit in `causal_review.rs`, the regime-boundary
  uncertainty, and the causal digest (`central/causal.rs`, which filters on
  `is_decayed` and not on retirement). This is chosen, not overlooked.
  Removing an edge from the shared-cause cap is loosening a control
  automatically on the platform's own evidence — rule 53's shape with
  direct evidence in place of counterfactual — and §9.4's fourth handling
  states the asymmetry that decides it: a wrong edge kept costs a
  suboptimal allocation, a wrong edge released could cost an unbounded
  position. Each whole-graph reader's owner decides whether and how to honour
  `is_retired_by`; the digest's owner in particular is named under "What would make this wrong".
- **It does not change `failing_their_regime`.** A retired edge is the
  most-failing kind and stays in that count, which is what the causal review
  prints. No separate retired figure reaches the stage detail without a
  kernel change; the journal and `statistics()` carry it instead.

## What it costs

- A retired supply-chain claim is a mechanism claim refuted by a test that
  proposes no mechanism. The demo seed's hand-written edges can be retired
  by the precedence pass. That is honest — the platform's only test of the
  pair refused them three passes running — and the remedy is to re-claim
  with evidence, which writes a new edge.
- Three consecutive failures on a sliding window is correlated evidence. A
  bad stretch of tape can retire a true edge; the record survives and the
  link can be re-established, but a propagation in between runs without it.
- A retired driver still caps exposure in the shared-cause producer. That is
  conservative and can be wrong in the over-constraining direction.
- The causal digest still publishes a retired edge as active until its owner
  filters. Today no cell reads the digest's edge list — the kernel's own
  module doc for the belief slot says the same of that slot — so nothing
  sizes on it; the cost is a misleading count on the wire.

## Alternatives rejected

- **Attenuate the strength on failure.** That is "patched". The prior lane
  refused it for `decayed_at` and the reason is the same here: every
  propagation moves with nothing in the record naming the number that
  changed.
- **Drop the edge.** Loses the record a re-established link is judged
  against, and makes a replayed graph disagree with the log.
- **Retire on the first `KnownToFail`.** Makes two of §9.4's events one, and
  fires on a single window.
- **Retire from every reader, including the shared-cause cap and the
  digest.** Loosens a control automatically from evidence, and reaches two
  other lanes' files this wave.
- **Tie N to the window length so runs are independent.** Sixty-one daily
  passes is a quarter in one regime; most would reset before firing, and a
  control that cannot fire reads as protection and is not.
- **Express retirement as re-estimation decay.** Decay is "no evidence
  inside the horizon"; retirement is "evidence against". Two facts, two
  marks.

## What would make this wrong

1. When the precedence pass records the window it tested over, the count
   becomes failures over non-overlapping windows, and the constant's doc
   comment and the pin test move with it.
2. If the shared-cause owner decides a retired driver should release its
   cap, that is an amendment naming the approval path under rule 53, not a
   filter added in passing.
3. If any cell begins to read the causal digest's `active_edges` rather than
   the slot's freshness, the digest producer must honour `is_retired_by`
   first; until then the seam is named for its owner.

## Evidence

- `cargo test -p qip-world-model --test retirement` — ten tests, each
  asserting its premise before its property; the mutation report is in the
  lane's handback and commit messages.
- Production path: `grep -n 'record_causal_condition_failure' backend/crates/runtime/qip-kernel/src/platform.rs`
  (the UNDERSTAND-stage writer), `grep -n '\.explanations(' backend/crates/runtime/qip-kernel/src/platform.rs`
  and `grep -n 'second_order_exposure(\|hidden_concentration(' backend/crates/runtime/qip-kernel/src/platform.rs backend/crates/runtime/qip-kernel/src/causal_review.rs`
  (the readers a retired edge leaves).
- What still sees it: `grep -rn 'causal()\.edges()\|graph\.edges()\|causal\.edges()' backend/crates/runtime/qip-kernel/src`.
