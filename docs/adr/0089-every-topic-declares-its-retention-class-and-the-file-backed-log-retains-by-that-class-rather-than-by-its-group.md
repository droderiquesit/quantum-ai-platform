# ADR 0089: Every topic declares its retention class, and the file-backed log retains by that class rather than by its group

- **Status**: Accepted, under the authority the owner delegated on 2026-09-19
  to the lane driving §56.4's data rules; built in the commits this record
  travels with.
- **Date**: 2026-09-20
- **Supersedes**: nothing. Amends the placement half of ADR 0057 §4, which put
  `RetentionClass` in `qip-data-finder`; the type's rows, words and policies
  are unchanged and ADR 0057's test that reads them off the blueprint table
  still passes through a re-export.
- **Related**: ADR 0057 (the class as a type, and the fallback series), ADR
  0002 and the boundary rule that a lib may not depend on a service, ADR 0043
  (what the chain does and does not prove across an eviction).

## Context

Blueprint §56.4 rule 33: "Every retained byte belongs to a declared retention
class. Data with no class does not get written." §22.1 tables the nine classes.

ADR 0057 made the table a type, `RetentionClass`, and put it in
`qip-data-finder` beside the one class that needed a structure of its own,
the fallback bar series. That was the right crate for the series and the
wrong one for the vocabulary, and the register said so for a week: "the
event log still retains by topic group without naming a class per record,
which is the half of 'every stored byte carries a declared class' not yet
met."

The event log is where this platform's retained bytes live, and it is a lib
(`qip-events`). A lib may not depend on a service, so the log could not read
the class, and a class the one retaining structure cannot read is a class
nothing retains by. What the log actually retained by was
`Topic::requires_permanent_retention`, computed from `Topic::group` — four
groups permanent, two topics named by exception — and
`Topic::is_lossy_tolerable`, a four-topic list beside it. Two consequences
of retaining by *stage* rather than by *what the record is*:

- The kill switch's engagement was permanent and its release was not. Both
  are System-group topics; one was on the exception list. A full log would
  evict the record that said trading resumed and keep the one that said it
  stopped.
- A topic added to the Sense group was an observation by default, and one
  added to Reason, Decide, Act or Learn was permanent by default, with
  nothing in the type system asking which row of §22.1 it belonged to.
  Silence was a class.

## Decision

### 1. The table moves down into `qip-events`, and `qip-data-finder` re-exports it

`RetentionClass`, `Retention` and `FALLBACK_RETENTION` now live in
`qip_events::retention`
(`grep -n 'pub enum RetentionClass' backend/crates/libs/qip-events/src/retention.rs`).
`qip_data_finder::retention` re-exports the three names
(`grep -n 'pub use qip_events::retention' backend/crates/services/qip-data-finder/src/retention.rs`)
and keeps `FallbackSeries`, so no caller changes and ADR 0057's
blueprint-table test runs unchanged against the moved type. The dependency
direction is unchanged: `qip-events` depends on `qip-core` and serde only.

### 2. Every topic declares its row, exhaustively

`Topic::retention_class` is a `const fn` with an arm for all seventy-nine
topics and no wildcard —
`sed -n '/pub const fn retention_class/,/^    }$/p' backend/crates/libs/qip-events/src/topic.rs | grep -c '_ =>'`
prints `0` — so a topic added without a class is a compile error. That is
rule 33's second sentence held by the type system rather than by a reviewer.

The two predicates the streaming router, the mesh and the older tests ask are
**derived** from the class rather than listed beside it:
`is_lossy_tolerable` is `retention_class().is_replaceable()` and
`requires_permanent_retention` is `retention_class().is_permanent()`. One
declaration, not two lists that can drift.

Every assignment is the §22.1 row whose "what" column names the thing the
topic carries. Three are readings rather than quotations and are stated here
so that a reader can disagree with the reading rather than discover it:

- **The Understand group is *derived state***, not *semantic*. §22.1's
  semantic row — "entities, relations, causal edges, beliefs" — is retained
  indefinitely *by the world model*, which is the structure that holds that
  memory. The log's record of an `EntityResolved` is emitted per resolution
  (`grep -n 'const TOPIC: Topic = Topic::EntityResolved' backend/crates/services/qip-entity-resolution/src/entity.rs`),
  and a log that kept every resolution for ever would refuse the next fill to
  keep one — the append refusal is the designed backstop for the audit trail,
  not for a change stream that is rebuilt from the observations that produced
  it. Derived state is "in memory, fixed size regardless of throughput", which
  is what the world model's state is. The consequence is a tier change: these
  four topics were observations (evicted only after every replaceable record)
  and are now replaceable (rolled by the snapshot window when a caller sets
  one, spent first under pressure). `FeatureComputed` was already both.
- **The Simulate group and `DataQualityFailed` are *compact derived***:
  "solver deltas, counterfactual scores" and a per-source failure count are
  series, not observations. Their tier is unchanged.
- **Every System topic is *irreplaceable***: only this platform has its own
  lifecycle and control record. This is the tier change in the keeping
  direction — `ServiceStarted`, `ServiceStopped`, `KillSwitchReleased`,
  `BudgetExhausted` and `SystemAlert` were evictable and are now permanent,
  and the release and the engagement of the kill switch are one tier. These
  are low-volume topics; the test named below holds the release.

Everything else quotes its row: raw ticks, quotes and book deltas are
transient; a bar is the fallback series'; a trade, a filing, a macro print, a
story, an alternative datum, a reference-data update and the platform's own
manifest of a fetch are referenced (the story literally carries the manifest
with the content hash that row names); hypotheses and evidence are semantic;
signals, outcomes, attributions and lessons are episodic; every verdict,
order, fill, position and policy shipment is irreplaceable.

### 3. The log reads the class at both seams and nothing else

`EventLog::make_room` and `EventLog::roll` read `topic.retention_class()`
(`grep -n 'retention_class()' backend/crates/libs/qip-events/src/log.rs` —
three reads and the refusal that names the incoming record's class). What
the policy decides:

- `Retention::is_replaceable` — `Never` and `InMemoryFixed`. The record is
  the log's working set: rolled by age behind the snapshot window and the
  first thing spent under pressure. Both rows say the structure holding the
  fact is bounded by something other than the log.
- `Retention::is_permanent` — `Permanent` and `Indefinite`. Never evicted;
  when nothing else remains the append is refused.
- Everything between — `ManifestOnly`, `Series`, `For`, `Rolling` — is an
  observation: kept until pressure has spent every replaceable record, never
  rolled by age, because the structure the row names is what retains it and
  the log is only its index.

Two things are deliberately **not** changed. The snapshot window stays off
unless a caller sets it, for the reason `qip_events::log`'s module doc gives
(two consumers replay the retained span from genesis and refuse a gap). And
the `Rolling(90 days)` policy's own duration is not yet read by the log: no
topic declares `EventAnchored` — book state at an own order is a fact only a
cell holds, and §22.1's row for it is the execution plane's to fill, as row
22.1 of the register says — so a roll keyed on that duration would be a
control with nothing to act on.

### 4. What reads it, and what proves it

The non-test call path is every composition root's journal:
`EventLog::append` → `make_room` and `roll_if_due` → the class. Three tests
in `backend/crates/libs/qip-events/tests/backbone.rs`, each mutation-verified
with the source restored by `sha256sum` afterwards:

- `every_topic_declares_the_retention_class_its_record_carries` — the
  reviewer's copy of the table, and the two derived predicates agreeing
  with it. Fired when `NewsReceived` was swapped to `Transient`.
- `a_topics_declared_retention_class_and_not_its_group_decides_whether_the_log_rolls_it`
  — a tick and a story share the Sense group; the roll takes the tick and
  leaves the story, and a fill is untouched. Fired on the property when
  `NewsReceived` was swapped to `Transient`, and again when `roll` was made
  to read `group().is_latency_critical()` in place of the class.
- `the_kill_switchs_release_is_as_permanent_as_its_engagement` — a full log
  of releases refuses a tick rather than evicting one. Fired when
  `make_room`'s second pass was made to evict regardless of permanence.

## What it costs

- **Two tier changes**, both named in §2: the Understand group's change
  stream becomes replaceable, and the System group becomes permanent. Neither
  reaches a deployed process, because no composition root sets a snapshot
  window and none has yet filled a log to its ceiling; both are visible in
  `every_topic_declares_the_retention_class_its_record_carries`.
- **The refusal message changed shape** to name the incoming record's class;
  it keeps the phrase "permanent retention" two older tests match on.
- **`qip-events` gains a module** (`retention.rs`) and one exhaustive
  seventy-nine-arm match that every new topic must extend. That is the point.

## What would make this wrong

- **A topic whose honest §22.1 row is not the tier the log should give it.**
  `Retention` decides the tier and the class decides the retention; if a
  record needs a tier its row does not imply, that is a tenth row for §22.1
  and an amendment here, not a wildcard arm.
- **A writer of `EventAnchored` lands.** Then the roll should honour the
  row's own ninety days rather than the caller's window, and §3's second
  "not changed" reverses.
- **A second retaining store with no class.** The key-value store — a
  connector's checkpoint and stream ledger, the lifecycle's trials, the mesh
  spine's spool
  (`grep -rln 'dyn KeyValueStore' --include=*.rs backend/crates/services backend/crates/runtime | grep -v /tests/`)
  — writes bytes under no declared class today. Rule 33's second sentence
  would be held there by `KeyValueStore::put` taking a class and refusing
  without one; that is `qip-core`'s seam and a separate decision.

## Consequences

Rule 33 is held for the event log by construction and for the fallback
series by ADR 0057; the key-value store is the named residue. The register's
row 56.4 says so with the commands above.
